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
use std::sync::{Arc, Mutex};

use corelens_mcp::{bridge, Backend, BoxFuture, Outcome};
use rusqlite::Connection;
use serde_json::{json, Map, Value};

use super::fake_device::{FakeDevice, PASSWORD};
use super::{execute, Activity, Host};
use crate::commands::AppState;
use crate::db;

const MODERN: &str = "2026-07-28";

/// The binary cargo built beside this test. Unit tests are not given
/// `CARGO_BIN_EXE_*`, but `cargo test` builds the binary for the
/// integration tests in the same run, next to `deps/`.
fn binary() -> PathBuf {
    let deps = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let exe = deps.parent().unwrap().join(format!("corelens-desktop-ai{}", std::env::consts::EXE_SUFFIX));
    assert!(exe.exists(), "{} is not built; run `cargo test -p corelens-desktop-ai` so the binary is built with the tests", exe.display());
    exe
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
fn app_state() -> AppState {
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
    db::save_credential(&conn, &login, 1).unwrap();
    db::set_credential_owner(&conn, "cred-1", Some("p1")).unwrap();
    let mut document = super::project::fixture::document();
    document["pages"][0]["nodes"][0]["data"]["addresses"][0]["address"] = json!("127.0.0.1");
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
        vault_key: Mutex::new(Some(key)),
        limiter: crate::ratelimit::RateLimiter::default(),
        sessions: Arc::new(crate::terminal::Sessions::default()),
        export_targets: crate::commands::ExportTargets::default(),
        open_project: Mutex::new(Some("p1".into())),
    }
}

/// The window's end of the bridge, with the fake switch's port to dial.
struct Window {
    state: AppState,
    ssh_port: u16,
}

impl Backend for Window {
    fn call<'a>(&'a self, tool: &'a str, arguments: Value) -> BoxFuture<'a, Outcome> {
        Box::pin(async move {
            let emit = |_: &Activity| {};
            let templates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../resources/templates/ntc");
            let host = Host { state: &self.state, templates: Some(templates), ssh_port: self.ssh_port, emit: &emit };
            execute(&host, tool, arguments).await
        })
    }
}

/// One session with the binary: the requests in, every reply out. Stdin
/// stays open until each request has its answer, as a client keeps it
/// open; the server lets calls still running at end of input go.
fn session(home: &Path, requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(binary())
        .arg("--mcp")
        .env("XDG_DATA_HOME", home)
        .env("HOME", home)
        .env("LOCALAPPDATA", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary starts");
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    let wanted = requests.iter().filter(|r| r.get("id").is_some()).count();
    let mut replies: Vec<Value> = Vec::new();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    while replies.len() < wanted {
        let Some(line) = lines.next() else { break };
        replies.push(serde_json::from_str(&line.unwrap()).expect("every stdout line is an MCP message"));
    }
    drop(stdin);
    assert!(lines.next().is_none(), "nothing more after the last answer");
    assert!(child.wait().unwrap().success());
    replies
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
            "device" => "SW1",
            "command" => "show ip arp",
            _ => "SW1",
        }),
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

async fn with_window<T: Send + 'static>(run: impl FnOnce(PathBuf) -> T + Send + 'static) -> (T, FakeDevice) {
    let home = tempfile::tempdir().unwrap();
    let dir = data_dir(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    let dev = FakeDevice::start().await;
    let listening = bridge::listen(bridge::endpoint(&dir), Arc::new(Window { state: app_state(), ssh_port: dev.port })).await.unwrap();
    let h = home.path().to_path_buf();
    let out = tokio::task::spawn_blocking(move || run(h)).await.unwrap();
    listening.stop().await;
    drop(home);
    (out, dev)
}

#[tokio::test(flavor = "multi_thread")]
async fn run_show_command_reaches_the_switch_from_a_spec_shaped_call() {
    let calls = [(1, "run_show_command", json!({ "device": "SW1", "command": "show ip arp" }))];
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
        (1, "run_show_command", json!({ "device": "SW1" })),
        (2, "run_show_command", json!({ "device": "SW1", "command": "show ip arp", "secretish": "value-never-echoed" })),
        (3, "get_config", json!({ "device": "SW1", "from_line": "value-never-echoed" })),
    ];
    let (replies, dev) = with_window(move |home| session(&home, &legacy(&calls))).await;
    let one = text(reply(&replies, 1));
    assert!(one.starts_with("run_show_command:") && one.contains("missing field `command`") && one.contains("Keys received: device."), "{one}");
    let two = text(reply(&replies, 2));
    assert!(two.starts_with("run_show_command:") && two.contains("Keys received: command, device, secretish."), "{two}");
    let three = text(reply(&replies, 3));
    assert!(three.starts_with("get_config:") && three.contains("Keys received: device, from_line."), "{three}");
    for r in [one, two, three] {
        assert!(!r.contains("value-never-echoed"), "a value was echoed: {r}");
    }
    assert!(dev.asked().iter().all(|c| c != "show ip arp"), "a refused call reached the switch");
}
