//! The whole way, as an MCP client drives it: the shipped binary started
//! with `--mcp`, spec-shaped `tools/call` requests on its stdin, the local
//! bridge, and this module's own `execute` behind it with a real database,
//! an unlocked vault, an open project and a fake switch to log in to.
//!
//! Every tool's arguments are built from the `inputSchema` the binary
//! advertises, every property filled in, so a schema and the handler that
//! reads it cannot drift apart without one of these failing.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use corelens_mcp::{bridge, Backend, BoxFuture, Outcome};
use rusqlite::Connection;
use serde_json::{json, Map, Value};

use super::fake_device::{FakeDevice, PASSWORD};
use super::changes::{Change, ChangeHub, ChangeStatus, Decision};
use super::{execute, Activity, Host, Source};
use crate::commands::AppState;
use crate::db;

const MODERN: &str = "2026-07-28";

/// The binary cargo built with this test. Unit tests are not given
/// `CARGO_BIN_EXE_*`, but `cargo test` builds the binary for the
/// integration tests in the same run, in the profile's folder. The test
/// itself sits below that folder — in `deps/` on older Cargo, under
/// `build/<package>/<hash>/` from Rust 1.100 — so the folders above it are
/// searched rather than one assumed.
fn binary() -> PathBuf {
    let name = format!("corelens-desktop-ai{}", std::env::consts::EXE_SUFFIX);
    let here = std::env::current_exe().unwrap();
    here.ancestors()
        .skip(1)
        .map(|dir| dir.join(&name))
        .find(|exe| exe.is_file())
        .unwrap_or_else(|| panic!("no {name} above {}; run `cargo test -p corelens-desktop-ai` so the binary is built with the tests", here.display()))
}

/// The data folder the binary will look in, under a scratch home.
fn data_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Corelens Desktop AI")
    } else {
        home.join("Corelens Desktop AI")
    }
}

/// The app's state as the window has it once the engineer has opened the
/// project and unlocked the vault: one switch, its login saved.
pub(super) fn app_state() -> AppState {
    app_state_as("SW1", true)
}

/// The same, with the switch under another name and the vault either
/// unlocked or still locked.
pub(crate) fn app_state_as(label: &str, unlocked: bool) -> AppState {
    let conn = Connection::open_in_memory().unwrap();
    db::migrate_for_tests(&conn);
    let (header, key) = corelens_discover::vault::create("fixture vault passphrase").unwrap();
    db::create_vault(&conn, &header.salt, &header.verifier, 1).unwrap();
    let sealed = corelens_discover::vault::seal(&key, PASSWORD).unwrap();
    let login = db::StoredCredential {
        id: "cred-1".into(),
        label: "reader".into(),
        kind: "ssh".into(),
        username: "reader".into(),
        secret: (sealed.nonce, sealed.ciphertext),
        extra: None,
        detail: String::new(),
    };
    db::save_credential(&conn, "p1", &login, 1).unwrap();
    let mut document = super::project::fixture::document();
    document["pages"][0]["nodes"][0]["data"]["addresses"][0]["address"] = json!("127.0.0.1");
    document["pages"][0]["nodes"][0]["data"]["label"] = json!(label);
    let package: db::ProjectPackage = serde_json::from_value(json!({
        "meta": { "id": "p1", "name": "Example project", "customer": "Example Co", "site": "HQ", "created_at": 1, "updated_at": 1 },
        "document_version": 1,
        "document": document
    }))
    .unwrap();
    db::upsert_project(&conn, &package).unwrap();
    let (engine, _rx) = corelens_probe::Engine::new(4);
    AppState {
        engine,
        db: Mutex::new(conn),
        session_id: Mutex::new(None),
        project_id: Mutex::new(None),
        jobs: Arc::new(crate::jobs::Jobs::default()),
        vault_key: Mutex::new(unlocked.then_some(key)),
        limiter: crate::ratelimit::RateLimiter::default(),
        sessions: Arc::new(crate::terminal::Sessions::default()),
        export_targets: crate::commands::ExportTargets::default(),
        open_project: Mutex::new(Some("p1".into())),
    }
}

/// The window's end of the bridge, with the fake switch's port to dial,
/// the hub proposals wait in, and every change it told the page about.
pub(super) struct Window {
    pub(super) state: AppState,
    pub(super) ssh_port: u16,
    pub(super) hub: Arc<ChangeHub>,
    pub(super) shown: Arc<Mutex<Vec<Change>>>,
    /// What the engineer clicks for every proposal, in a test that does
    /// not decide them itself: deny, so nothing is ever pushed by accident.
    pub(super) approve_all: bool,
}

impl Window {
    pub(super) fn new(state: AppState, ssh_port: u16) -> Window {
        Window { state, ssh_port, hub: Arc::new(ChangeHub::default()), shown: Arc::new(Mutex::new(Vec::new())), approve_all: false }
    }
}

impl Backend for Window {
    fn call<'a>(&'a self, tool: &'a str, arguments: Value, client: Option<&'a str>) -> BoxFuture<'a, Outcome> {
        Box::pin(async move {
            let emit = |_: &Activity| {};
            let shown = Arc::clone(&self.shown);
            let hub = Arc::clone(&self.hub);
            let approve_all = self.approve_all;
            let emit_change = move |c: &Change| {
                shown.lock().unwrap().push(c.clone());
                if c.status == ChangeStatus::Pending && hub.is_waiting(&c.id) {
                    let d = if approve_all { Decision::Approve(c.targets.iter().map(|t| t.seq).collect()) } else { Decision::Deny("denied by the test".into()) };
                    let _ = hub.decide(&c.id, d);
                }
            };
            let templates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../resources/templates/ntc");
            let host = Host {
                state: &self.state,
                templates: Some(templates),
                ssh_port: self.ssh_port,
                emit: &emit,
                changes: Some(&self.hub),
                emit_change: &emit_change,
                source: Source::Mcp { client: client.map(str::to_string) },
                app: None,
            };
            execute(&host, tool, arguments).await
        })
    }
}

/// The longest a session may take: every call is bounded well inside
/// this (a show command by its own limit, the bridge by its call limit),
/// so passing it means a reply was lost, not that a device was slow.
const SESSION_LIMIT: Duration = Duration::from_secs(300);

/// One session with the binary: the requests in, every reply out. Stdin
/// stays open until each request has its answer, as a client keeps it
/// open; the server lets calls still running at end of input go. A reply
/// that never comes fails the test with the ids still missing and the
/// binary's own log of what it received, rather than waiting for ever.
fn session(home: &Path, requests: &[Value]) -> Vec<Value> {
    session_logged(home, requests).0
}

/// The same, with what the binary wrote to its debug log.
fn session_logged(home: &Path, requests: &[Value]) -> (Vec<Value>, String) {
    let log = home.join("stderr.log");
    let mut child = Command::new(binary())
        .arg("--mcp")
        .env("XDG_DATA_HOME", home)
        .env("HOME", home)
        .env("LOCALAPPDATA", home)
        .env("CORELENS_MCP_DEBUG", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .expect("the binary starts");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    let mut waiting: Vec<Value> = requests.iter().filter_map(|r| r.get("id").cloned()).collect();
    let deadline = Instant::now() + SESSION_LIMIT;
    let mut replies: Vec<Value> = Vec::new();
    while !waiting.is_empty() {
        let line = match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "no reply within {}s to ids {waiting:?}; the binary's log:\n{}",
                    SESSION_LIMIT.as_secs(),
                    std::fs::read_to_string(&log).unwrap_or_default()
                );
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let r: Value = serde_json::from_str(&line).expect("every stdout line is an MCP message");
        waiting.retain(|id| *id != r["id"]);
        replies.push(r);
    }
    drop(stdin);
    let status = child.wait().unwrap();
    reader.join().unwrap();
    let rest: Vec<String> = rx.try_iter().collect();
    assert!(rest.is_empty(), "nothing more after the last answer: {rest:?}");
    assert!(status.success(), "the binary exited with {status}");
    (replies, std::fs::read_to_string(&log).unwrap_or_default())
}

fn reply(replies: &[Value], id: i64) -> &Value {
    replies.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("no reply {id} in {replies:?}"))
}

fn text(reply: &Value) -> &str {
    reply["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no text in {reply}"))
}

/// A value for a property of the advertised schema.
fn sample(key: &str, schema: &Value) -> Value {
    match schema["type"].as_str() {
        Some("integer") => schema.get("minimum").cloned().unwrap_or(json!(1)),
        Some("string") => json!(match key {
            "target" => "SW1",
            "command" => "show ip arp",
            "reason" => "a round-trip test",
            "change_id" => "00000000",
            _ => "SW1",
        }),
        Some("array") => match schema["items"]["type"].as_str() {
            Some("string") => match key {
                "commands" => json!(["interface Gi1/0/24", " description LAB-PORT"]),
                "verify_commands" => json!(["show interfaces status"]),
                _ => json!(["SW1"]),
            },
            Some("object") => json!([arguments(&schema["items"], false)]),
            other => panic!("{key}: an array of {other:?} needs a sample here"),
        },
        other => panic!("{key}: a property of type {other:?} needs a sample here"),
    }
}

/// Arguments for a tool: every property its schema advertises, or only
/// the required ones.
fn arguments(schema: &Value, all: bool) -> Value {
    let required: Vec<&str> = schema["required"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut out = Map::new();
    for (k, v) in schema["properties"].as_object().unwrap() {
        if all || required.contains(&k.as_str()) {
            out.insert(k.clone(), sample(k, v));
        }
    }
    Value::Object(out)
}

/// The requests a client of the `initialize` era sends, as Claude Desktop
/// does, then the calls.
fn legacy(calls: &[(i64, &str, Value)]) -> Vec<Value> {
    let mut out = vec![
        json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test-client", "version": "1" } } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    ];
    for (id, name, args) in calls {
        out.push(json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": args } }));
    }
    out
}

/// The same calls in the stateless revision.
fn modern(calls: &[(i64, &str, Value)]) -> Vec<Value> {
    let meta = json!({ "io.modelcontextprotocol/protocolVersion": MODERN, "io.modelcontextprotocol/clientCapabilities": {} });
    calls
        .iter()
        .map(|(id, name, args)| json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "_meta": meta, "name": name, "arguments": args } }))
        .collect()
}

/// On Windows the bridge is one pipe per user, so the tests that open it
/// take turns; elsewhere each has its own socket and this costs nothing.
static BRIDGE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn with_window<T: Send + 'static>(run: impl FnOnce(PathBuf) -> T + Send + 'static) -> (T, FakeDevice) {
    with_window_over(app_state(), run).await
}

async fn with_window_over<T: Send + 'static>(state: AppState, run: impl FnOnce(PathBuf) -> T + Send + 'static) -> (T, FakeDevice) {
    let _turn = BRIDGE.lock().await;
    let home = tempfile::tempdir().unwrap();
    let dir = data_dir(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    let dev = FakeDevice::start().await;
    let listening = bridge::listen(bridge::endpoint(&dir), Arc::new(Window::new(state, dev.port))).await.unwrap();
    let h = home.path().to_path_buf();
    let out = tokio::task::spawn_blocking(move || run(h)).await.unwrap();
    listening.stop().await;
    drop(home);
    (out, dev)
}

#[tokio::test(flavor = "multi_thread")]
async fn run_show_command_reaches_the_switch_from_a_spec_shaped_call() {
    let calls = [(1, "run_show_command", json!({ "target": "SW1", "command": "show ip arp" }))];
    let ((old, new), dev) = with_window(move |home| (session(&home, &legacy(&calls)), session(&home, &modern(&calls)))).await;
    for replies in [&old, &new] {
        let r = reply(replies, 1);
        assert_eq!(r["result"]["isError"], false, "{r}");
        assert!(text(r).contains("Internet  192.0.2.1"), "{r}");
    }
    assert_eq!(dev.asked().iter().filter(|c| *c == "show ip arp").count(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_tool_reads_the_arguments_its_schema_advertises() {
    let tools = corelens_mcp::tools::list();
    let mut calls = Vec::new();
    let mut id = 1;
    for t in tools.as_array().unwrap() {
        let schema = &t["inputSchema"];
        if schema["properties"].as_object().is_none_or(|p| p.is_empty()) {
            continue;
        }
        let name = t["name"].as_str().unwrap().to_string();
        for all in [false, true] {
            calls.push((id, name.clone(), arguments(schema, all)));
            id += 1;
        }
    }
    assert!(calls.len() >= 14, "every tool with arguments is covered: {calls:?}");
    let borrowed: Vec<(i64, &str, Value)> = calls.iter().map(|(i, n, a)| (*i, n.as_str(), a.clone())).collect();
    let requests = [legacy(&borrowed), modern(&borrowed.iter().map(|(i, n, a)| (i + 1000, *n, a.clone())).collect::<Vec<_>>())];
    let (replies, _dev) = with_window(move |home| requests.iter().map(|r| session(&home, r)).collect::<Vec<_>>()).await;
    for (replies, offset) in replies.iter().zip([0, 1000]) {
        for (i, name, args) in &calls {
            let r = reply(replies, i + offset);
            assert!(r.get("error").is_none(), "{name} {args}: a protocol error: {r}");
            let t = text(r);
            assert!(!t.contains("do not match its schema"), "{name} {args}: the handler refused what the schema advertises: {t}");
            if *name == "run_show_command" || *name == "get_device" {
                assert_eq!(r["result"]["isError"], false, "{name} {args}: {r}");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_call_names_the_tool_and_the_keys_never_the_values() {
    let calls = [
        (1, "run_show_command", json!({ "target": "SW1" })),
        (2, "run_show_command", json!({ "target": "SW1", "command": "show ip arp", "secretish": "value-never-echoed" })),
        (3, "get_config", json!({ "target": "SW1", "from_line": "value-never-echoed" })),
        (4, "run_show_command", json!({ "command": "show version" })),
        (5, "get_device", json!({})),
    ];
    let (replies, dev) = with_window(move |home| session(&home, &legacy(&calls))).await;
    let one = text(reply(&replies, 1));
    assert!(one.starts_with("run_show_command:") && one.contains("missing field `command`") && one.contains("Keys received: target."), "{one}");
    let two = text(reply(&replies, 2));
    assert!(two.starts_with("run_show_command:") && two.contains("Keys received: command, secretish, target."), "{two}");
    let three = text(reply(&replies, 3));
    assert!(three.starts_with("get_config:") && three.contains("Keys received: from_line, target."), "{three}");
    let four = text(reply(&replies, 4));
    assert_eq!(four, "run_show_command: missing 'target' (the device name or IP). Keys received: command.");
    let five = text(reply(&replies, 5));
    assert_eq!(five, "get_device: missing 'target' (the device name or IP). Keys received: none.");
    for r in [one, two, three] {
        assert!(!r.contains("value-never-echoed"), "a value was echoed: {r}");
    }
    assert!(dev.asked().iter().all(|c| c != "show ip arp"), "a refused call reached the switch");
}

/// A call as Claude Desktop sends it: the initialize era, a `_meta` with
/// a progress token beside the name, the arguments as an object.
fn desktop_call(id: i64, name: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments, "_meta": { "progressToken": id } } })
}

fn desktop_session(calls: Vec<Value>) -> Vec<Value> {
    let mut out = vec![
        json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": { "extensions": {} }, "clientInfo": { "name": "claude-ai", "version": "0.1.0" } } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 900, "method": "tools/list", "params": {} }),
    ];
    out.extend(calls);
    out
}

/// Claude Desktop's own shape, a device named with hyphens, `show
/// version`, eight at once alongside reads of the same device: each one
/// reaches the switch and none loses its arguments on the way.
#[tokio::test(flavor = "multi_thread")]
async fn claude_desktops_call_in_parallel_keeps_its_arguments() {
    let name = "EXAMPLE-MAIN-SW";
    let mut calls = Vec::new();
    for i in 1..=8 {
        calls.push(desktop_call(i, "run_show_command", json!({ "target": name, "command": "show version" })));
        calls.push(desktop_call(100 + i, "get_device", json!({ "target": name })));
    }
    let requests = desktop_session(calls);
    let ((replies, log), dev) = with_window_over(app_state_as(name, true), move |home| session_logged(&home, &requests)).await;
    for i in 1..=8 {
        let r = reply(&replies, i);
        assert_eq!(r["result"]["isError"], false, "{r}\nthe binary's log:\n{log}");
        assert!(text(r).contains(name) && text(r).contains("show version"), "{r}");
        let d = reply(&replies, 100 + i);
        assert_eq!(d["result"]["isError"], false, "{d}\nthe binary's log:\n{log}");
    }
    assert!(dev.asked().iter().filter(|c| *c == "show version").count() >= 8, "{:?}", dev.asked());
}

/// With the vault still locked the answer says so, for every call at
/// once, and never that the arguments were wrong.
#[tokio::test(flavor = "multi_thread")]
async fn a_locked_vault_is_said_plainly_and_never_as_missing_arguments() {
    let name = "EXAMPLE-MAIN-SW";
    let calls = (1..=6).map(|i| desktop_call(i, "run_show_command", json!({ "target": name, "command": "show version" }))).collect();
    let requests = desktop_session(calls);
    let (replies, dev) = with_window_over(app_state_as(name, false), move |home| session(&home, &requests)).await;
    for i in 1..=6 {
        let r = reply(&replies, i);
        assert_eq!(r["result"]["isError"], true, "{r}");
        let t = text(r);
        assert!(t.contains("vault in Corelens is locked") && !t.contains("missing field") && !t.contains("do not match its schema"), "{t}");
    }
    assert!(dev.asked().is_empty(), "a locked vault still reached the switch: {:?}", dev.asked());
}
