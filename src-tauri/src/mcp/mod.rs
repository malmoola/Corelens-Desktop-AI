//! Claude — or any MCP client — using Corelens, from the app's side.
//!
//! The client starts `corelens-desktop-ai --mcp`, which speaks MCP and
//! forwards each tool call over a local, authenticated bridge
//! (`corelens_mcp::bridge`) to this window. Here the call is checked
//! against the open project, run, redacted, written to the audit log and
//! answered. Off by default: nothing listens until Settings → AI turns it
//! on, and turning it off removes the endpoint and its key.

pub mod audit;
pub mod changes;
#[cfg(test)]
pub mod fake_device;
pub mod project;
pub mod redact;
#[cfg(test)]
pub(crate) mod roundtrip;
pub mod show;
pub mod tools;

use std::sync::Arc;

use corelens_mcp::{Backend, BoxFuture, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::{open_project, AppState};
use crate::db;
use crate::vault_commands::db_err;

type CmdResult<T> = Result<T, String>;

/// The app setting that keeps the bridge on across restarts.
const SETTING: &str = "mcpEnabled";

pub use audit::when as audit_time;

/// The bridge's state in the window.
#[derive(Default)]
pub struct McpState {
    listening: tokio::sync::Mutex<Option<corelens_mcp::bridge::Listening>>,
    error: std::sync::Mutex<Option<String>>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Every secret the unlocked vault holds, so a result can be cleaned of
/// them by value. Nothing when locked: a locked vault opens no session
/// either, so no device output can carry one of its secrets.
pub(crate) fn vault_secrets(state: &AppState) -> redact::Secrets {
    let Ok(guard) = state.vault_key.lock() else { return redact::Secrets::default() };
    let Some(key) = guard.as_ref() else { return redact::Secrets::default() };
    let Ok(conn) = state.db.lock() else { return redact::Secrets::default() };
    let ids: Vec<String> = conn
        .prepare("SELECT id FROM credentials")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0)).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    let mut values = Vec::new();
    for id in ids {
        let Ok(Some(stored)) = db::credential(&conn, &id) else { continue };
        if let Ok(s) = crate::vault_commands::open_secret(key, &stored.secret) {
            values.push(s);
        }
        if let Some(extra) = &stored.extra {
            if let Ok(s) = crate::vault_commands::open_secret(key, extra) {
                values.push(s);
            }
        }
    }
    redact::Secrets::new(values)
}

/// What the window's status strip shows while a call runs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    tool: String,
    device: String,
    running: bool,
    outcome: String,
}

/// A tool's raw result and what it touched, before the gate.
pub(crate) struct Raw {
    pub(crate) result: Result<String, String>,
    pub(crate) device: String,
    pub(crate) address: String,
    pub(crate) command: String,
}

/// The device, the address to dial, and the logins to try, in order.
type Prepared = (project::Device, String, Vec<(String, corelens_discover::ssh::Credentials)>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShowArgs {
    target: String,
    command: String,
}

/// Who is asking: an MCP client (by the name it gave), the in-app chat
/// (its provider and model), or the engineer in this window.
#[derive(Debug, Clone)]
pub enum Source {
    Mcp { client: Option<String> },
    Chat { provider: String, model: String },
    Operator,
}

impl Source {
    pub fn kind(&self) -> &'static str {
        match self {
            Source::Mcp { .. } => "mcp",
            Source::Chat { .. } => "chat",
            Source::Operator => "operator",
        }
    }

    /// As the change record names the proposer.
    pub fn describe(&self) -> String {
        match self {
            Source::Mcp { client: Some(c) } => format!("{c} (MCP)"),
            Source::Mcp { client: None } => "an MCP client".into(),
            Source::Chat { provider, model } => format!("Corelens chat ({provider}, {model})"),
            Source::Operator => "the engineer".into(),
        }
    }
}

/// What a call needs from the window: its state, where the parser
/// templates are, the port SSH sessions dial, where to say a call has
/// started or ended (the status strip), and — for a change — the hub its
/// decision comes through, where to tell the page about it, and who asked.
pub struct Host<'a> {
    pub state: &'a AppState,
    pub templates: Option<std::path::PathBuf>,
    pub ssh_port: u16,
    pub emit: &'a (dyn Fn(&Activity) + Send + Sync),
    /// `None` where changes cannot be decided (no window).
    pub changes: Option<&'a changes::ChangeHub>,
    pub emit_change: &'a (dyn Fn(&changes::Change) + Send + Sync),
    pub source: Source,
    /// For work that outlives the call (a commit's timer).
    pub app: Option<AppHandle>,
}

/// The device a call names, the address to dial, and the logins to try:
/// the device's own first, then the project's.
pub(crate) fn login_for(state: &AppState, project_id: &str, target: &str) -> Result<Prepared, String> {
    let package = {
        let conn = state.db.lock().map_err(db_err)?;
        db::load_project(&conn, project_id).map_err(db_err)?.ok_or("The project open in Corelens could not be read.")?
    };
    let all = project::devices(&package.document);
    let device = project::resolve(&all, target)?.clone();
    let address = device.target().ok_or_else(|| format!("{} has no address in the project, so Corelens cannot reach it.", device.label()))?;
    if state.vault_key.lock().map(|k| k.is_none()).unwrap_or(true) {
        return Err("The vault in Corelens is locked, so no login can be used. The engineer unlocks it in the Corelens window, then ask again.".into());
    }
    let first = Some(device.field("sshCredentialId").to_string())
        .filter(|c| !c.is_empty() && crate::vault_commands::credential_exists(state, c))
        .or_else(|| crate::vault_commands::project_login_order(state, &[], None).into_iter().find(|id| crate::vault_commands::credential_exists(state, id)))
        .ok_or_else(|| format!("No login is saved for {} or for the project, so Corelens cannot log in to it.", device.label()))?;
    let chain = crate::vault_commands::ssh_login_chain(state, &first)?;
    Ok((device, address, chain))
}

async fn run_show(host: &Host<'_>, project_id: &str, arguments: &Value) -> Raw {
    let mut raw = Raw { result: Err(String::new()), device: String::new(), address: String::new(), command: String::new() };
    let args: ShowArgs = match tools::args("run_show_command", arguments) {
        Ok(a) => a,
        Err(e) => {
            raw.result = Err(e);
            return raw;
        }
    };
    raw.command = args.command.trim().to_string();
    let state = host.state;
    let prepared = show::precheck(&args.command).and_then(|_| login_for(state, project_id, &args.target));
    let (device, address, chain) = match prepared {
        Ok(p) => p,
        Err(e) => {
            raw.result = Err(e);
            return raw;
        }
    };
    raw.device = device.label().to_string();
    raw.address = address.clone();
    let store = match crate::discovery::load_host_keys(state) {
        Ok(s) => s,
        Err(e) => {
            raw.result = Err(e);
            return raw;
        }
    };
    let logins: Vec<_> = chain.iter().map(|(_, c)| c.clone()).collect();
    for (id, _) in chain.iter().take(1) {
        crate::vault_commands::note_use(state, id, "Claude: read-only show command", &address);
    }
    let ran = show::run(device.label(), &address, host.ssh_port, &logins, &args.command, Arc::clone(&store), host.templates.as_deref()).await;
    crate::discovery::persist_host_keys_to(state, &store);
    raw.result = ran.map(|r| r.text);
    raw
}

fn run_read(state: &AppState, project_id: &str, tool: &str, arguments: &Value) -> Raw {
    let device = arguments.get("target").and_then(Value::as_str).unwrap_or("").to_string();
    let result = (|| -> Result<String, String> {
        // The vault before the database, as everywhere else, or two calls
        // at once can each hold the lock the other is waiting for.
        let vault_unlocked = state.vault_key.lock().map(|k| k.is_some()).unwrap_or(false);
        let conn = state.db.lock().map_err(db_err)?;
        let package = db::load_project(&conn, project_id).map_err(db_err)?.ok_or("The project open in Corelens could not be read.")?;
        let backup_root = db::project_settings(&conn, project_id).map_err(db_err)?.get("backupFolder").cloned().map(std::path::PathBuf::from);
        let ctx = tools::Ctx { conn: &conn, project: &package, backup_root, vault_unlocked };
        tools::run(&ctx, tool, arguments)
    })();
    Raw { result, device, address: String::new(), command: String::new() }
}

/// One call from a client, start to finish.
pub async fn execute(host: &Host<'_>, tool: &str, arguments: Value) -> Outcome {
    let started = now_ms();
    let state = host.state;
    let project_id = open_project(state);
    let device_arg = arguments.get("target").and_then(Value::as_str).unwrap_or("").to_string();
    (host.emit)(&Activity { tool: tool.into(), device: device_arg.clone(), running: true, outcome: String::new() });
    let raw = match &project_id {
        None => Raw { result: Err("No project is open in Corelens. The engineer opens the customer's project in the Corelens window; then ask again.".into()), device: device_arg, address: String::new(), command: String::new() },
        Some(p) if tool == "run_show_command" => run_show(host, p, &arguments).await,
        Some(p) if tool == "propose_config_change" => changes::propose(host, p, &arguments).await,
        Some(p) if tool == "propose_save_config" => changes::propose_save(host, p, &arguments).await,
        Some(_) if tool == "get_change_status" => changes::status(host, &arguments).await,
        Some(p) => run_read(state, p, tool, &arguments),
    };
    let secrets = vault_secrets(state);
    let (outcome, kind) = match raw.result {
        Ok(text) => match redact::gate(&text, &secrets) {
            Ok(clean) => (Outcome::ok(corelens_mcp::tools::cap(clean)), "ok"),
            Err(redact::Withheld) => (Outcome::error(redact::WITHHELD), "withheld"),
        },
        Err(message) => match redact::gate(&message, &secrets) {
            Ok(clean) => (Outcome::error(clean), "error"),
            Err(redact::Withheld) => (Outcome::error(redact::WITHHELD), "withheld"),
        },
    };
    let entry = audit::Entry {
        at_ms: started,
        project_id: project_id.clone(),
        tool: tool.into(),
        arguments: redact::gate(&arguments.to_string(), &secrets).unwrap_or_else(|_| "{withheld}".into()),
        device: raw.device.clone(),
        address: raw.address,
        command: raw.command,
        outcome: kind.into(),
        result: outcome.text.clone(),
        bytes: outcome.text.len() as i64,
        duration_ms: now_ms() - started,
        ..Default::default()
    };
    if let Ok(conn) = state.db.lock() {
        let _ = audit::record(&conn, &entry);
        let _ = audit::prune(&conn, now_ms());
    }
    crate::applog::verbose_with("mcp", || format!("{tool} via {}: {kind}, {} bytes, {} ms", host.source.kind(), entry.bytes, entry.duration_ms));
    (host.emit)(&Activity { tool: tool.into(), device: raw.device, running: false, outcome: kind.into() });
    outcome
}

struct AppBackend {
    app: AppHandle,
}

impl Backend for AppBackend {
    fn call<'a>(&'a self, tool: &'a str, arguments: Value, client: Option<&'a str>) -> BoxFuture<'a, Outcome> {
        Box::pin(async move {
            let state = self.app.state::<AppState>();
            let hub = self.app.state::<changes::ChangeHub>();
            let emit = |a: &Activity| {
                let _ = self.app.emit("corelens://mcp-activity", a);
            };
            let emit_change = |c: &changes::Change| {
                let _ = self.app.emit(changes::EVENT, c);
            };
            let host = Host {
                state: state.inner(),
                templates: crate::collection::templates_dir(&self.app).ok(),
                ssh_port: 22,
                emit: &emit,
                changes: Some(hub.inner()),
                emit_change: &emit_change,
                source: Source::Mcp { client: client.map(str::to_string) },
                app: Some(self.app.clone()),
            };
            execute(&host, tool, arguments).await
        })
    }
}

async fn start(app: &AppHandle) -> Result<(), String> {
    let mcp = app.state::<McpState>();
    let mut slot = mcp.listening.lock().await;
    if slot.is_some() {
        return Ok(());
    }
    let endpoint = corelens_mcp::bridge::endpoint(&db::data_dir());
    let backend: Arc<dyn Backend> = Arc::new(AppBackend { app: app.clone() });
    let result = corelens_mcp::bridge::listen(endpoint, backend).await.map_err(|e| format!("The AI bridge could not start: {e}"));
    let error = result.as_ref().err().cloned();
    if let Ok(l) = result {
        *slot = Some(l);
    }
    if let Ok(mut e) = mcp.error.lock() {
        e.clone_from(&error);
    }
    error.map_or(Ok(()), Err)
}

async fn stop(app: &AppHandle) {
    let mcp = app.state::<McpState>();
    let taken = mcp.listening.lock().await.take();
    if let Some(l) = taken {
        l.stop().await;
    }
}

fn enabled(state: &AppState) -> bool {
    state.db.lock().ok().and_then(|c| db::all_settings(&c).ok()).and_then(|s| s.get(SETTING).cloned()).as_deref() == Some("1")
}

/// Starts the bridge at launch when it was left on.
pub fn on_startup(app: &AppHandle) {
    if enabled(&app.state::<AppState>()) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = start(&app).await;
        });
    }
}

/// What Settings → AI shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatus {
    pub enabled: bool,
    pub listening: bool,
    pub error: Option<String>,
    /// This installation's executable, as a client must be told to start it.
    pub executable: String,
    /// `claude mcp add …` for Claude Code, ready to paste.
    pub claude_code_command: String,
    /// The `mcpServers` entry for Claude Desktop's configuration file.
    pub claude_desktop_config: String,
}

/// The setup text for an executable at `exe`.
pub fn setup_text(exe: &str) -> (String, String) {
    let quoted = if exe.contains(' ') { format!("\"{exe}\"") } else { exe.to_string() };
    let command = format!("claude mcp add --transport stdio --scope user corelens -- {quoted} --mcp");
    let desktop = serde_json::to_string_pretty(&json!({ "mcpServers": { "corelens": { "command": exe, "args": ["--mcp"] } } })).unwrap_or_default();
    (command, desktop)
}

async fn status_of(app: &AppHandle) -> McpStatus {
    let state = app.state::<AppState>();
    let mcp = app.state::<McpState>();
    let listening = mcp.listening.lock().await.is_some();
    let executable = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let (claude_code_command, claude_desktop_config) = setup_text(&executable);
    McpStatus { enabled: enabled(&state), listening, error: mcp.error.lock().ok().and_then(|e| e.clone()), executable, claude_code_command, claude_desktop_config }
}

#[tauri::command]
pub async fn mcp_status(app: AppHandle) -> CmdResult<McpStatus> {
    Ok(status_of(&app).await)
}

/// Turns the bridge on or off, and remembers which.
#[tauri::command]
pub async fn mcp_set_enabled(app: AppHandle, enabled: bool) -> CmdResult<McpStatus> {
    {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err(db_err)?;
        db::set_setting(&conn, SETTING, if enabled { Some("1") } else { None }).map_err(db_err)?;
    }
    if enabled {
        // A failure to listen is reported in the status, not as an error:
        // the switch stays on, and the reason is shown beside it.
        let _ = start(&app).await;
    } else {
        stop(&app).await;
        if let Ok(mut e) = app.state::<McpState>().error.lock() {
            *e = None;
        }
    }
    Ok(status_of(&app).await)
}

/// The audit log, newest first: the open project's, or every project's.
#[tauri::command(async)]
pub fn mcp_activity(state: State<'_, AppState>, all_projects: bool, limit: Option<i64>) -> CmdResult<Vec<audit::Entry>> {
    let project = if all_projects { None } else { open_project(&state) };
    if !all_projects && project.is_none() {
        return Ok(Vec::new());
    }
    let conn = state.db.lock().map_err(db_err)?;
    audit::list(&conn, project.as_deref(), limit.unwrap_or(200).clamp(1, 5000)).map_err(db_err)
}

/// Clears the open project's audit log, or every project's.
#[tauri::command(async)]
pub fn mcp_clear_activity(state: State<'_, AppState>, all_projects: bool) -> CmdResult<usize> {
    let project = if all_projects { None } else { open_project(&state) };
    if !all_projects && project.is_none() {
        return Ok(0);
    }
    let conn = state.db.lock().map_err(db_err)?;
    audit::clear(&conn, project.as_deref()).map_err(db_err)
}

/// The open project's audit log as a change record (Markdown), for the
/// page to save through the native dialog.
#[tauri::command(async)]
pub fn mcp_change_record(state: State<'_, AppState>) -> CmdResult<String> {
    let project = open_project(&state).ok_or("Open the project whose record you want first.")?;
    let conn = state.db.lock().map_err(db_err)?;
    let name = db::load_project(&conn, &project).map_err(db_err)?.map(|p| p.meta.name).unwrap_or_default();
    let entries = audit::list(&conn, Some(&project), 100_000).map_err(db_err)?;
    Ok(audit::change_record(&name, &entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setup_text_starts_this_executable_with_mcp() {
        let (cmd, desktop) = setup_text(r"C:\Program Files\Corelens Desktop AI\corelens-desktop-ai.exe");
        assert_eq!(cmd, r#"claude mcp add --transport stdio --scope user corelens -- "C:\Program Files\Corelens Desktop AI\corelens-desktop-ai.exe" --mcp"#);
        let v: Value = serde_json::from_str(&desktop).unwrap();
        assert_eq!(v["mcpServers"]["corelens"]["command"], r"C:\Program Files\Corelens Desktop AI\corelens-desktop-ai.exe");
        assert_eq!(v["mcpServers"]["corelens"]["args"], json!(["--mcp"]));
        assert_eq!(setup_text("/usr/bin/corelens-desktop-ai").0, "claude mcp add --transport stdio --scope user corelens -- /usr/bin/corelens-desktop-ai --mcp");
    }

    /// Calls run side by side, and each takes the vault and the database.
    /// Taken in opposite orders, two calls each hold what the other waits
    /// for and the database is never let go. The order is the vault, then
    /// the database: a call waiting for the vault must not be sitting on
    /// the database meanwhile.
    #[test]
    fn a_call_waiting_for_the_vault_is_not_holding_the_database() {
        let state = Arc::new(super::roundtrip::app_state());
        let held = state.vault_key.lock().unwrap();
        let call = {
            let state = Arc::clone(&state);
            std::thread::spawn(move || {
                let emit = |_: &Activity| {};
                let emit_change = |_: &changes::Change| {};
                let host = Host { state: &state, templates: None, ssh_port: 22, emit: &emit, changes: None, emit_change: &emit_change, source: Source::Operator, app: None };
                tauri::async_runtime::block_on(execute(&host, "get_device", json!({ "target": "SW1" })))
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        let free = state.db.try_lock().is_ok();
        drop(held);
        let outcome = call.join().unwrap();
        assert!(free, "the call took the database and then waited for the vault");
        assert!(!outcome.is_error, "{}", outcome.text);
    }
}
