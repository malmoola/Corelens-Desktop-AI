//! The in-app AI chat and the providers it runs on.
//!
//! **Off by default.** Until *AI chat in Corelens* is switched on and a
//! provider is configured, nothing here makes a single network call: no
//! model list is fetched, no connection is tested, no turn is sent. Every
//! request goes out from Rust ([`provider::send`]); the page sends text and
//! receives text, never a key or a request.
//!
//! One tool layer: a chat turn calls the same `mcp::execute` an MCP
//! client reaches, so the project scope, the allowlists, redaction, the
//! audit record and the tier-2 approval are exactly the ones MCP has.

pub mod anthropic;
pub mod chat;
pub mod keys;
pub mod ollama;
pub mod openai;
pub mod provider;
pub mod transcript;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

use self::provider::{Provider, ProviderError};
use crate::commands::{open_project, AppState};
use crate::db;
use crate::vault_commands::db_err;

type CmdResult<T> = Result<T, String>;

pub const EVENT: &str = "corelens://ai-chat";

pub const PROVIDERS: &[&str] = &["anthropic", "openai", "ollama"];

/// The settings the page may change, and only these.
const KEYS: &[&str] = &[
    "aiChatEnabled",
    "aiProvider",
    "aiModel.anthropic",
    "aiModel.openai",
    "aiModel.ollama",
    "aiOpenAiBaseUrl",
    "aiOpenAiStyle",
    "aiOllamaBaseUrl",
    "aiMaxToolCalls",
    "aiToolOutputChars",
    transcript::RETENTION_SETTING,
    crate::mcp::audit::RETENTION_SETTING,
    crate::mcp::changes::TIMEOUT_SETTING,
];

/// What the window holds for the chat: the turn running per project (so
/// Stop can end it) and the last request body sent to each provider, for
/// the redaction check in Settings.
#[derive(Default)]
pub struct AiState {
    running: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
    last_body: Mutex<HashMap<String, Value>>,
}

impl AiState {
    /// Ends every turn still being answered, for a change of project, and
    /// says whose they were.
    pub fn stop_all(&self) -> Vec<String> {
        let Ok(mut running) = self.running.lock() else { return Vec::new() };
        let mut stopped: Vec<String> = running.drain().filter(|(_, h)| !h.inner().is_finished()).map(|(p, h)| {
            h.abort();
            p
        }).collect();
        stopped.sort();
        stopped
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettings {
    pub chat_enabled: bool,
    pub provider: String,
    pub models: HashMap<String, String>,
    pub openai_base_url: String,
    pub openai_style: String,
    pub ollama_base_url: String,
    pub max_tool_calls: usize,
    pub tool_output_chars: usize,
    pub chat_retention_days: i64,
    pub audit_retention_days: i64,
    pub proposal_timeout_minutes: i64,
    /// Whether a key is kept for each provider that takes one. Never the
    /// key.
    pub keys: HashMap<String, bool>,
}

fn settings_map(state: &AppState) -> HashMap<String, String> {
    state.db.lock().ok().and_then(|c| db::all_settings(&c).ok()).unwrap_or_default()
}

fn settings_from(s: &HashMap<String, String>, keys_present: HashMap<String, bool>) -> AiSettings {
    let num = |k: &str, d: usize, lo: usize, hi: usize| s.get(k).and_then(|v| v.parse::<usize>().ok()).unwrap_or(d).clamp(lo, hi);
    let days = |k: &str, d: i64| s.get(k).and_then(|v| v.parse::<i64>().ok()).filter(|v| [30, 90, 365].contains(v)).unwrap_or(d);
    AiSettings {
        chat_enabled: s.get("aiChatEnabled").map(String::as_str) == Some("1"),
        provider: s.get("aiProvider").cloned().filter(|p| PROVIDERS.contains(&p.as_str())).unwrap_or_else(|| "anthropic".into()),
        models: PROVIDERS
            .iter()
            .map(|p| {
                let default = match *p {
                    "anthropic" => "claude-opus-5-5",
                    "openai" => "gpt-6-astra",
                    _ => "qwen3",
                };
                (p.to_string(), s.get(&format!("aiModel.{p}")).cloned().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| default.into()))
            })
            .collect(),
        openai_base_url: s.get("aiOpenAiBaseUrl").cloned().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| openai::API.into()),
        openai_style: s.get("aiOpenAiStyle").cloned().filter(|v| v == "chat").unwrap_or_else(|| "responses".into()),
        ollama_base_url: s.get("aiOllamaBaseUrl").cloned().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| ollama::DEFAULT_BASE.into()),
        max_tool_calls: num("aiMaxToolCalls", chat::DEFAULT_MAX_TOOL_CALLS, 1, 50),
        tool_output_chars: num("aiToolOutputChars", chat::DEFAULT_TOOL_OUTPUT_CHARS, 2_000, 24_000),
        chat_retention_days: days(transcript::RETENTION_SETTING, transcript::DEFAULT_RETENTION_DAYS),
        audit_retention_days: days(crate::mcp::audit::RETENTION_SETTING, crate::mcp::audit::DEFAULT_RETENTION_DAYS),
        proposal_timeout_minutes: s.get(crate::mcp::changes::TIMEOUT_SETTING).and_then(|v| v.parse::<i64>().ok()).unwrap_or(crate::mcp::changes::DEFAULT_TIMEOUT_MIN).clamp(1, crate::mcp::changes::MAX_TIMEOUT_MIN),
        keys: keys_present,
    }
}

fn current(state: &AppState) -> AiSettings {
    let present = ["anthropic", "openai"].iter().map(|p| (p.to_string(), keys::present(p))).collect();
    settings_from(&settings_map(state), present)
}

/// A base URL the engineer typed: http or https, no credentials in it.
fn check_url(url: &str) -> Result<String, String> {
    let u = url.trim().trim_end_matches('/');
    if !(u.starts_with("https://") || u.starts_with("http://")) {
        return Err("A base URL starts with https:// (or http:// for a server on this network).".into());
    }
    if u.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("").contains('@') {
        return Err("A base URL must not carry a user name or password; the key is kept in the keychain.".into());
    }
    Ok(u.to_string())
}

/// The provider a turn runs on, built from the settings and the keychain.
/// Refused while the chat is off: nothing reaches a provider then.
pub fn build(settings: &AiSettings, which: &str) -> Result<Box<dyn Provider>, String> {
    if !settings.chat_enabled {
        return Err("AI chat in Corelens is off. Switch it on in Settings → AI first.".into());
    }
    let model = settings.models.get(which).cloned().unwrap_or_default();
    match which {
        "anthropic" => {
            let key = keys::read("anthropic")?.ok_or("No Anthropic API key is kept yet: copy one and press Paste key in Settings → AI → AI providers.")?;
            Ok(Box::new(anthropic::Anthropic::new(key, model)))
        }
        "openai" => {
            let key = keys::read("openai")?.ok_or("No OpenAI API key is kept yet: copy one and press Paste key in Settings → AI → AI providers.")?;
            let style = if settings.openai_style == "chat" { openai::Style::Chat } else { openai::Style::Responses };
            Ok(Box::new(openai::OpenAi { key, model, base: settings.openai_base_url.clone(), style }))
        }
        "ollama" => Ok(Box::new(ollama::Ollama { model, base: settings.ollama_base_url.clone() })),
        _ => Err(format!("{which} is not a provider Corelens knows.")),
    }
}

#[tauri::command(async)]
pub fn ai_settings(state: State<'_, AppState>) -> CmdResult<AiSettings> {
    Ok(current(state.inner()))
}

/// Changes one setting; `None` clears it back to its default.
#[tauri::command(async)]
pub fn ai_setting_set(state: State<'_, AppState>, key: String, value: Option<String>) -> CmdResult<AiSettings> {
    if !KEYS.contains(&key.as_str()) {
        return Err(format!("{key} is not an AI setting."));
    }
    let value = match (key.as_str(), value) {
        (_, None) => None,
        ("aiOpenAiBaseUrl" | "aiOllamaBaseUrl", Some(v)) if v.trim().is_empty() => None,
        ("aiOpenAiBaseUrl" | "aiOllamaBaseUrl", Some(v)) => Some(check_url(&v)?),
        ("aiProvider", Some(v)) if PROVIDERS.contains(&v.as_str()) => Some(v),
        ("aiProvider", Some(v)) => return Err(format!("{v} is not a provider Corelens knows.")),
        ("aiChatEnabled", Some(v)) => (v == "1").then(|| "1".to_string()),
        (_, Some(v)) => Some(v.trim().chars().take(200).collect()),
    };
    {
        let conn = state.db.lock().map_err(db_err)?;
        db::set_setting(&conn, &key, value.as_deref()).map_err(db_err)?;
    }
    crate::applog::info("ai", &format!("setting {key} changed"));
    Ok(current(state.inner()))
}

/// Reads the provider's key from the clipboard into the keychain.
#[tauri::command(async)]
pub fn ai_key_import(state: State<'_, AppState>, provider: String) -> CmdResult<AiSettings> {
    keys::import_from_clipboard(&provider)?;
    Ok(current(state.inner()))
}

#[tauri::command(async)]
pub fn ai_key_forget(state: State<'_, AppState>, provider: String) -> CmdResult<AiSettings> {
    keys::forget(&provider)?;
    Ok(current(state.inner()))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    /// Whether it calls tools, where the provider says (Ollama).
    pub tools: Option<bool>,
}

async fn list_models(settings: &AiSettings, which: &str) -> Result<Vec<ModelInfo>, String> {
    let p = build(settings, which)?;
    match which {
        "anthropic" => {
            let key = keys::read("anthropic")?.unwrap_or_default();
            let v = provider::get(p.as_ref(), &anthropic::models_url(anthropic::API), &anthropic::Anthropic::new(key, String::new()).headers()).await.map_err(|e| e.to_string())?;
            Ok(anthropic::models_from(&v).into_iter().map(|(id, label)| ModelInfo { id, label, tools: Some(true) }).collect())
        }
        "openai" => {
            let o = openai::OpenAi { key: keys::read("openai")?.unwrap_or_default(), model: String::new(), base: settings.openai_base_url.clone(), style: openai::Style::Responses };
            let v = provider::get(p.as_ref(), &format!("{}/models", settings.openai_base_url.trim_end_matches('/')), &o.headers()).await.map_err(|e| e.to_string())?;
            Ok(openai::models_from(&v).into_iter().map(|(id, label)| ModelInfo { id, label, tools: None }).collect())
        }
        _ => {
            let o = ollama::Ollama { model: String::new(), base: settings.ollama_base_url.clone() };
            let v = provider::get(p.as_ref(), &o.tags_url(), &[]).await.map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for (id, size) in ollama::models_from(&v) {
                let show = provider::post_json(p.as_ref(), &o.show_url(), &serde_json::json!({ "model": id })).await.ok();
                out.push(ModelInfo { tools: show.as_ref().and_then(ollama::calls_tools), label: size, id });
            }
            Ok(out)
        }
    }
}

/// The provider's models, asked for when the engineer presses Refresh.
#[tauri::command]
pub async fn ai_models(state: State<'_, AppState>, provider: String) -> CmdResult<Vec<ModelInfo>> {
    let settings = current(state.inner());
    list_models(&settings, &provider).await
}

/// Asks the provider one small thing and says what came back.
#[tauri::command]
pub async fn ai_test_connection(state: State<'_, AppState>, provider: String) -> CmdResult<String> {
    let settings = current(state.inner());
    let model = settings.models.get(&provider).cloned().unwrap_or_default();
    let models = list_models(&settings, &provider).await?;
    let found = models.iter().find(|m| m.id == model);
    Ok(match (provider.as_str(), found) {
        ("ollama", Some(m)) if m.tools == Some(false) => ProviderError::NoTools(format!("Connected, but the Ollama model {model} cannot call tools, which the chat needs. Pull a model that can — {} — and choose it.", ollama::TOOL_MODELS)).to_string(),
        (_, Some(_)) => format!("Connected: {} models available, {model} among them.", models.len()),
        ("openai", None) if settings.openai_base_url != openai::API => format!("Connected: {} models listed; {model} is not among them, which an OpenAI-compatible server may still accept.", models.len()),
        (_, None) => format!("Connected, but {model} is not among the {} models this account can use. Choose one of them.", models.len()),
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    pub rows: Vec<transcript::Row>,
    pub session: provider::Usage,
    pub running: bool,
}

/// The open project's conversation.
#[tauri::command(async)]
pub fn ai_chat_history(state: State<'_, AppState>, ai: State<'_, AiState>, limit: Option<i64>) -> CmdResult<History> {
    let Some(project) = open_project(&state) else { return Ok(History { rows: Vec::new(), session: Default::default(), running: false }) };
    let conn = state.db.lock().map_err(db_err)?;
    let rows = transcript::list(&conn, &project, limit.unwrap_or(500).clamp(1, 5000)).map_err(db_err)?;
    let session = transcript::totals(&conn, &project).map_err(db_err)?;
    let running = ai.running.lock().map(|r| r.get(&project).is_some_and(|h| !h.inner().is_finished())).unwrap_or(false);
    Ok(History { rows, session, running })
}

/// Clears the open project's conversation, or every project's.
#[tauri::command(async)]
pub fn ai_chat_clear(state: State<'_, AppState>, all_projects: bool) -> CmdResult<usize> {
    let project = if all_projects { None } else { Some(open_project(&state).ok_or("Open a project first.")?) };
    let conn = state.db.lock().map_err(db_err)?;
    transcript::clear(&conn, project.as_deref()).map_err(db_err)
}

/// Sends the engineer's message and runs the turn; what happens arrives on
/// [`EVENT`]. `context` names what the question is about (a device, a
/// link, a finding), put in front of the message.
#[tauri::command]
pub async fn ai_chat_send(app: AppHandle, text: String, context: Option<String>) -> CmdResult<()> {
    let state = app.state::<AppState>();
    let project = open_project(&state).ok_or("Open a project first: the chat works on the project open in Corelens.")?;
    let settings = current(state.inner());
    let which = settings.provider.clone();
    // Built here so a missing key or a chat that is off is said at once.
    let provider = build(&settings, &which)?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Type a question first.".into());
    }
    let message = match context.map(|c| c.trim().chars().take(500).collect::<String>()).filter(|c| !c.is_empty()) {
        Some(c) => format!("[About: {c}]\n{text}"),
        None => text,
    };
    {
        let ai = app.state::<AiState>();
        let running = ai.running.lock().map_err(|_| "busy")?;
        if running.get(&project).is_some_and(|h| !h.inner().is_finished()) {
            return Err("A question is still being answered; wait for it, or press Stop.".into());
        }
    }
    let app2 = app.clone();
    let pid = project.clone();
    let handle = tauri::async_runtime::spawn(async move {
        let state = app2.state::<AppState>();
        let hub = app2.state::<crate::mcp::changes::ChangeHub>();
        let emit_activity = |a: &crate::mcp::Activity| {
            let _ = app2.emit("corelens://mcp-activity", a);
        };
        let emit_change = |c: &crate::mcp::changes::Change| {
            let _ = app2.emit(crate::mcp::changes::EVENT, c);
        };
        let host = crate::mcp::Host {
            state: state.inner(),
            templates: crate::collection::templates_dir(&app2).ok(),
            ssh_port: 22,
            emit: &emit_activity,
            changes: Some(hub.inner()),
            emit_change: &emit_change,
            source: crate::mcp::Source::Chat { provider: which.clone(), model: provider.model().to_string() },
            app: Some(app2.clone()),
        };
        let emit = |e: chat::ChatEvent| {
            let _ = app2.emit(EVENT, chat::event_json(&pid, &e));
        };
        let keep_body = |b: &Value| {
            if let Ok(mut l) = app2.state::<AiState>().last_body.lock() {
                l.insert(which.clone(), b.clone());
            }
        };
        let cfg = chat::ChatConfig { provider: provider.as_ref(), max_tool_calls: settings.max_tool_calls, tool_output_chars: settings.tool_output_chars };
        if let Err(e) = chat::run_turn(&host, &pid, &cfg, &message, &emit, &keep_body).await {
            crate::applog::warn("chat", &format!("a turn on {which} ended with an error: {}", e.chars().take(200).collect::<String>()));
            emit(chat::ChatEvent::Error { message: e });
        }
    });
    if let Ok(mut r) = app.state::<AiState>().running.lock() {
        r.insert(project, handle);
    }
    Ok(())
}

/// Stops the open project's running turn. A proposal it was waiting on is
/// withdrawn.
#[tauri::command]
pub async fn ai_chat_stop(app: AppHandle) -> CmdResult<bool> {
    let state = app.state::<AppState>();
    let Some(project) = open_project(&state) else { return Ok(false) };
    let handle = app.state::<AiState>().running.lock().ok().and_then(|mut r| r.remove(&project));
    let Some(h) = handle else { return Ok(false) };
    h.abort();
    let _ = app.emit(EVENT, serde_json::json!({ "projectId": project, "kind": "stopped" }));
    Ok(true)
}

/// The last request body sent to a provider (after redaction; the key is
/// a header and is not in it), for the engineer to check in Settings.
#[tauri::command(async)]
pub fn ai_last_request(ai: State<'_, AiState>, provider: String) -> CmdResult<String> {
    let body = ai.last_body.lock().ok().and_then(|l| l.get(&provider).cloned());
    Ok(match body {
        Some(b) => {
            let text = serde_json::to_string_pretty(&b).unwrap_or_default();
            if text.len() > 400_000 {
                format!("{}\n… (cut at 400 000 characters)", &text[..text.char_indices().take_while(|(i, _)| *i < 400_000).last().map(|(i, _)| i).unwrap_or(0)])
            } else {
                text
            }
        }
        None => format!("Nothing has been sent to {provider} since Corelens started."),
    })
}
