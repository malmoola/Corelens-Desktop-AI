//! The chat against each provider's wire format, served from loopback: a
//! fake provider that answers a turn with a tool call and the next with
//! text, the real fake switch behind the tool, and every byte the
//! provider was sent read back.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::chat::{self, ChatConfig, ChatEvent};
use super::provider::*;
use super::{anthropic, ollama, openai};
use crate::mcp::changes::{store, ChangeHub, ChangeStatus};
use crate::mcp::fake_device::{FakeDevice, COMMUNITY, ENABLE_SECRET, PASSWORD};
use crate::mcp::{redact, Activity, Host, Source};

/// A provider that answers request N with `replies[N]` (the last one
/// again after that), and keeps every request body it was sent.
struct FakeProvider {
    base: String,
    bodies: Arc<Mutex<Vec<String>>>,
}

async fn fake_provider(content_type: &'static str, replies: Vec<String>) -> FakeProvider {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    tokio::spawn(async move {
        let mut n = 0usize;
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            // Headers, then as much body as Content-Length says.
            let body = loop {
                let Ok(k) = sock.read(&mut chunk).await else { break String::new() };
                if k == 0 {
                    break String::new();
                }
                buf.extend_from_slice(&chunk[..k]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text[..end].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                    if buf.len() >= end + 4 + len {
                        break String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).to_string();
                    }
                }
            };
            seen.lock().unwrap().push(body);
            let reply = replies.get(n).or(replies.last()).cloned().unwrap_or_default();
            n += 1;
            let _ = sock.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\nconnection: close\r\n\r\n{reply}").as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    FakeProvider { base, bodies }
}

fn sse(events: &[Value]) -> String {
    events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap_or("message"))).collect()
}

/// Anthropic: a call to read the running configuration, then an answer.
fn anthropic_replies(tool: &str, input: Value) -> Vec<String> {
    vec![
        sse(&[
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 100 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "toolu_1", "name": tool, "input": {} } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": input.to_string() } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 20 } }),
            json!({ "type": "message_stop" }),
        ]),
        sse(&[
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 300 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "SW1 is fine." } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 5 } }),
        ]),
    ]
}

fn openai_replies(tool: &str, input: Value) -> Vec<String> {
    vec![
        sse(&[
            json!({ "type": "response.output_item.added", "output_index": 0, "item": { "type": "function_call", "call_id": "call_1", "name": tool, "arguments": "" } }),
            json!({ "type": "response.output_item.done", "output_index": 0, "item": { "type": "function_call", "call_id": "call_1", "name": tool, "arguments": input.to_string() } }),
            json!({ "type": "response.completed", "response": { "usage": { "input_tokens": 100, "output_tokens": 20 } } }),
        ]),
        sse(&[
            json!({ "type": "response.output_text.delta", "item_id": "m", "output_index": 0, "content_index": 0, "delta": "SW1 is fine." }),
            json!({ "type": "response.output_item.done", "output_index": 0, "item": { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "SW1 is fine." }] } }),
            json!({ "type": "response.completed", "response": { "usage": { "input_tokens": 300, "output_tokens": 5 } } }),
        ]),
    ]
}

fn chat_completions_replies(tool: &str, input: Value) -> Vec<String> {
    vec![
        format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{ "index": 0, "id": "call_1", "type": "function", "function": { "name": tool, "arguments": input.to_string() } }] }, "finish_reason": "tool_calls" }] }),
            json!({ "choices": [], "usage": { "prompt_tokens": 100, "completion_tokens": 20 } })
        ),
        format!("data: {}\n\ndata: [DONE]\n\n", json!({ "choices": [{ "index": 0, "delta": { "content": "SW1 is fine." }, "finish_reason": "stop" }] })),
    ]
}

fn ollama_replies(tool: &str, input: Value) -> Vec<String> {
    vec![
        format!("{}\n{}\n", json!({ "message": { "role": "assistant", "content": "", "tool_calls": [{ "function": { "name": tool, "arguments": input } }] }, "done": false }), json!({ "message": { "role": "assistant", "content": "" }, "done": true, "done_reason": "stop", "prompt_eval_count": 100, "eval_count": 20 })),
        format!("{}\n{}\n", json!({ "message": { "role": "assistant", "content": "SW1 is fine." }, "done": false }), json!({ "message": { "role": "assistant", "content": "" }, "done": true, "prompt_eval_count": 300, "eval_count": 5 })),
    ]
}

struct Desk {
    dev: FakeDevice,
    state: crate::commands::AppState,
    hub: ChangeHub,
    events: Arc<Mutex<Vec<ChatEvent>>>,
}

async fn desk() -> Desk {
    Desk { dev: FakeDevice::start().await, state: crate::mcp::roundtrip::app_state_as("SW1", true), hub: ChangeHub::default(), events: Arc::new(Mutex::new(Vec::new())) }
}

impl Desk {
    async fn turn(&self, p: &dyn Provider, text: &str, max_tool_calls: usize) -> Result<(), String> {
        let activity = |_: &Activity| {};
        let hub = &self.hub;
        let emit_change = move |c: &crate::mcp::changes::Change| {
            if c.status == ChangeStatus::Pending && hub.is_waiting(&c.id) {
                let _ = crate::mcp::changes::decide_as_operator(&self.state, hub, &c.id, vec![0], &[], None);
            }
        };
        let host = Host { state: &self.state, templates: None, ssh_port: self.dev.port, emit: &activity, changes: Some(&self.hub), emit_change: &emit_change, source: Source::Chat { provider: p.id().into(), model: p.model().into() }, app: None };
        let events = Arc::clone(&self.events);
        let emit = move |e: ChatEvent| events.lock().unwrap().push(e);
        chat::run_turn(&host, "p1", &ChatConfig { provider: p, max_tool_calls, tool_output_chars: 12_000 }, text, &emit, &|_| {}).await
    }
}

fn config_with_secrets() -> String {
    format!(
        "hostname SW1\nenable secret 9 $9$fixturefixturefixture\nusername reader privilege 15 password 0 {PASSWORD}\nsnmp-server community {COMMUNITY} RO\n description the old one was {ENABLE_SECRET}\ncrypto key\n-----BEGIN RSA PRIVATE KEY-----\nMIIfixturefixture\n-----END RSA PRIVATE KEY-----\nkey sk-ant-api03-FIXTUREfixtureFIXTUREfixture\n"
    )
}

const SECRETS: &[&str] = &[PASSWORD, COMMUNITY, ENABLE_SECRET, "$9$fixture", "MIIfixturefixture", "sk-ant-api03-FIXTURE"];

#[test]
fn no_provider_request_builder_sends_a_secret_from_a_config_or_a_question() {
    let turn = Turn {
        system: chat::SYSTEM_PROMPT.into(),
        messages: vec![
            Message::User { text: format!("why does SW1 reject {PASSWORD}?") },
            Message::Assistant { text: String::new(), calls: vec![ToolCall { id: "c1".into(), name: "get_config".into(), arguments: json!({ "target": "SW1" }) }], native: None, provider: "x".into() },
            Message::Tools { results: vec![ToolResult { id: "c1".into(), name: "get_config".into(), content: config_with_secrets(), is_error: false }] },
        ],
        tools: chat::tool_defs(),
        max_tokens: 16_000,
    };
    let secrets = redact::Secrets::new(vec![PASSWORD.into(), ENABLE_SECRET.into()]);
    let key = || zeroize::Zeroizing::new("sk-proj-FIXTUREfixtureFIXTUREfixture0000".to_string());
    let providers: Vec<Box<dyn Provider>> = vec![
        Box::new(anthropic::Anthropic::new(zeroize::Zeroizing::new("sk-ant-api03-FIXTUREfixtureFIXTUREfixture".into()), "claude-opus-5-5".into())),
        Box::new(openai::OpenAi { key: key(), model: "gpt-6-astra".into(), base: openai::API.into(), style: openai::Style::Responses }),
        Box::new(openai::OpenAi { key: key(), model: "a-compatible-model".into(), base: "http://127.0.0.1:8000/v1".into(), style: openai::Style::Chat }),
        Box::new(ollama::Ollama { model: "qwen3".into(), base: ollama::DEFAULT_BASE.into() }),
    ];
    for p in &providers {
        let body = outbound_request(p.as_ref(), &turn, &secrets).body.to_string();
        for s in SECRETS {
            assert!(!body.contains(s), "{}: {s} is in the request body", p.id());
        }
        assert!(body.contains("hostname SW1"), "{}: the configuration itself is still sent", p.id());
        assert!(!body.contains("sk-proj-FIXTURE"), "{}: no key in a body", p.id());
    }
}

async fn whole_turn(p: &dyn Provider, fake: &FakeProvider, desk: &Desk) {
    desk.turn(p, "Is SW1 configured sanely?", 15).await.unwrap();
    let bodies = fake.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "{}: a call, then the answer", p.id());
    // What the vault holds and what the scrub recognises. (The fake's
    // description carries a fixture secret that is in neither, which no
    // gate can know to remove; the threat model says so.)
    for b in &bodies {
        for s in [PASSWORD, COMMUNITY, "$9$fixture"] {
            assert!(!b.contains(s), "{}: {s} reached the provider", p.id());
        }
    }
    assert!(bodies[1].contains("hostname SW1"), "{}: the tool's output went back to the model", p.id());
    let events = desk.events.lock().unwrap().clone();
    assert!(events.iter().any(|e| matches!(e, ChatEvent::ToolCall { name, target, command, .. } if name == "run_show_command" && target == "SW1" && command == "show running-config")), "{events:?}");
    assert!(events.iter().any(|e| matches!(e, ChatEvent::Text { text } if text.contains("SW1 is fine"))), "{events:?}");
    assert!(matches!(events.last(), Some(ChatEvent::Done { .. })), "{events:?}");
    let rows = super::transcript::list(&desk.state.db.lock().unwrap(), "p1", 100).unwrap();
    assert_eq!(rows.len(), 4, "the question, the call, the result, the answer");
    assert!(desk.dev.asked().contains(&"show running-config".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_on_anthropic_reads_a_config_and_sends_back_nothing_secret() {
    let desk = desk().await;
    let fake = fake_provider("text/event-stream", anthropic_replies("run_show_command", json!({ "target": "SW1", "command": "show running-config" }))).await;
    let mut p = anthropic::Anthropic::new(zeroize::Zeroizing::new("sk-ant-api03-FIXTUREfixtureFIXTUREfixture".into()), "claude-opus-5-5".into());
    p.base = fake.base.clone();
    whole_turn(&p, &fake, &desk).await;
    let second: Value = serde_json::from_str(&fake.bodies.lock().unwrap()[1]).unwrap();
    assert_eq!(second["messages"][1]["content"][0]["type"], "tool_use", "its own blocks sent back");
    assert_eq!(second["messages"][2]["content"][0]["tool_use_id"], "toolu_1");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_on_openai_responses_reads_a_config_and_sends_back_nothing_secret() {
    let desk = desk().await;
    let fake = fake_provider("text/event-stream", openai_replies("run_show_command", json!({ "target": "SW1", "command": "show running-config" }))).await;
    let p = openai::OpenAi { key: zeroize::Zeroizing::new("sk-proj-FIXTUREfixtureFIXTUREfixture0000".into()), model: "gpt-6-astra".into(), base: format!("{}/v1", fake.base), style: openai::Style::Responses };
    whole_turn(&p, &fake, &desk).await;
    let second: Value = serde_json::from_str(&fake.bodies.lock().unwrap()[1]).unwrap();
    assert!(second["input"].as_array().unwrap().iter().any(|i| i["type"] == "function_call_output" && i["call_id"] == "call_1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_on_an_openai_compatible_server_reads_a_config_and_sends_back_nothing_secret() {
    let desk = desk().await;
    let fake = fake_provider("text/event-stream", chat_completions_replies("run_show_command", json!({ "target": "SW1", "command": "show running-config" }))).await;
    let p = openai::OpenAi { key: zeroize::Zeroizing::new("sk-local-FIXTUREfixtureFIXTUREfixture".into()), model: "local".into(), base: format!("{}/v1", fake.base), style: openai::Style::Chat };
    whole_turn(&p, &fake, &desk).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_on_ollama_reads_a_config_and_sends_back_nothing_secret() {
    let desk = desk().await;
    let fake = fake_provider("application/x-ndjson", ollama_replies("run_show_command", json!({ "target": "SW1", "command": "show running-config" }))).await;
    let p = ollama::Ollama { model: "qwen3".into(), base: fake.base.clone() };
    whole_turn(&p, &fake, &desk).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_stops_at_its_tool_call_limit() {
    let desk = desk().await;
    // A model that never stops asking.
    let fake = fake_provider("application/x-ndjson", vec![ollama_replies("get_device", json!({ "target": "SW1" })).remove(0)]).await;
    let p = ollama::Ollama { model: "qwen3".into(), base: fake.base.clone() };
    desk.turn(&p, "loop", 2).await.unwrap();
    let events = desk.events.lock().unwrap().clone();
    let ran = events.iter().filter(|e| matches!(e, ChatEvent::ToolResult { is_error: false, .. })).count();
    assert_eq!(ran, 2, "{events:?}");
    assert!(events.iter().any(|e| matches!(e, ChatEvent::Notice { message } if message.contains("limit of 2"))), "{events:?}");
    assert!(fake.bodies.lock().unwrap().len() <= 4, "bounded");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_proposed_in_the_chat_waits_for_the_same_approval_and_says_who_asked() {
    let desk = desk().await;
    let fake = fake_provider("text/event-stream", anthropic_replies("propose_config_change", json!({ "target": "SW1", "commands": ["interface Gi1/0/24", " description FROM-CHAT"], "reason": "label it" }))).await;
    let mut p = anthropic::Anthropic::new(zeroize::Zeroizing::new("sk-ant-api03-FIXTUREfixtureFIXTUREfixture".into()), "claude-opus-5-5".into());
    p.base = fake.base.clone();
    desk.turn(&p, "label the spare port", 15).await.unwrap();
    let changes = store::list(&desk.state.db.lock().unwrap(), None, 10).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].source, "chat");
    assert_eq!(changes[0].proposer, "Corelens chat (anthropic, claude-opus-5-5)");
    assert!(desk.dev.running().iter().any(|l| l.contains("FROM-CHAT")), "approved in the test the way the window approves");
}

#[test]
fn nothing_is_built_while_the_chat_is_off() {
    let mut s = super::settings_from(&Default::default(), Default::default());
    assert!(!s.chat_enabled, "off by default");
    assert!(super::build(&s, "ollama").err().unwrap().contains("off"));
    s.chat_enabled = true;
    assert!(super::build(&s, "ollama").is_ok());
    assert_eq!(s.provider, "anthropic");
    assert_eq!(s.models["anthropic"], "claude-opus-5-5");
    assert_eq!(s.max_tool_calls, 15);
}

#[test]
fn a_base_url_is_http_and_carries_no_credentials() {
    assert_eq!(super::check_url("https://lab.example.test/v1/").unwrap(), "https://lab.example.test/v1");
    assert!(super::check_url("ftp://x").is_err());
    assert!(super::check_url("https://user:pass@lab.example.test").is_err());
}

// The log is one per process, so the tests that point it somewhere take
// turns, and this one holds its turn for the whole conversation.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread")]
async fn a_whole_turn_with_verbose_logging_on_leaves_no_secret_in_any_log() {
    let _turn = crate::applog::tests::TURN.lock().unwrap_or_else(|e| e.into_inner());
    let logs = tempfile::tempdir().unwrap();
    crate::applog::tests::into(logs.path());
    crate::applog::set_verbose_until(i64::MAX);
    let key = "sk-ant-api03-FIXTURElogFIXTURElogFIXTURE";
    crate::applog::remember_secret(key);
    let desk = desk().await;
    let fake = fake_provider("text/event-stream", anthropic_replies("run_show_command", json!({ "target": "SW1", "command": "show running-config" }))).await;
    let mut p = anthropic::Anthropic::new(zeroize::Zeroizing::new(key.into()), "claude-opus-5-5".into());
    p.base = fake.base.clone();
    desk.turn(&p, &format!("is {PASSWORD} the problem?"), 15).await.unwrap();
    // A failure, with the key and a secret in the error the code passes on.
    crate::applog::warn("ai", &format!("request failed for key {key} and login {PASSWORD}"));
    crate::applog::set_verbose_until(0);
    let all = crate::applog::tests::everything_written(logs.path());
    assert!(all.contains("anthropic claude-opus-5-5"), "verbose lines were written: {all}");
    for s in [PASSWORD, COMMUNITY, "$9$fixture", key] {
        assert!(!all.contains(s), "{s} reached the log:\n{all}");
    }
}
