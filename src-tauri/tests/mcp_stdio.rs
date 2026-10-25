//! The shipped binary as an MCP client starts it: `--mcp` on stdio, every
//! tool call carried over the local bridge to whatever is listening for
//! this user, and nothing on stdout but MCP.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;

use corelens_mcp::{bridge, Backend, BoxFuture, Outcome};
use serde_json::{json, Value};

struct Answers;
impl Backend for Answers {
    fn call<'a>(&'a self, tool: &'a str, arguments: Value, _client: Option<&'a str>) -> BoxFuture<'a, Outcome> {
        Box::pin(async move { Outcome::ok(format!("the app ran {tool} with {arguments}")) })
    }
}

/// The data folder the binary will look in, under a scratch home.
fn data_dir(home: &std::path::Path) -> std::path::PathBuf {
    if cfg!(windows) {
        home.join("Corelens Desktop AI")
    } else if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Corelens Desktop AI")
    } else {
        home.join("Corelens Desktop AI")
    }
}

fn spawn(home: &std::path::Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_corelens-desktop-ai"))
        .arg("--mcp")
        .env("XDG_DATA_HOME", home)
        .env("HOME", home)
        .env("LOCALAPPDATA", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary starts")
}

fn conversation(home: &std::path::Path) -> Vec<Value> {
    let mut child = spawn(home);
    let mut stdin = child.stdin.take().unwrap();
    let meta = json!({ "io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientCapabilities": {} });
    for (id, method, params) in [
        (1, "server/discover", json!({ "_meta": meta })),
        (2, "tools/list", json!({ "_meta": meta })),
        (3, "tools/call", json!({ "_meta": meta, "name": "get_device", "arguments": { "target": "SW1" } })),
    ] {
        writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).unwrap();
    }
    drop(stdin);
    let out = BufReader::new(child.stdout.take().unwrap());
    let replies: Vec<Value> = out.lines().map(|l| serde_json::from_str(&l.unwrap()).expect("every stdout line is an MCP message")).collect();
    assert!(child.wait().unwrap().success());
    replies
}

fn reply(replies: &[Value], id: i64) -> &Value {
    replies.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("no reply {id} in {replies:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_reaches_the_app_through_the_binary() {
    let home = tempfile::tempdir().unwrap();
    let dir = data_dir(home.path());
    std::fs::create_dir_all(&dir).unwrap();

    // Nothing listening: the protocol still answers, the call says why not.
    let h = home.path().to_path_buf();
    let replies = tokio::task::spawn_blocking(move || conversation(&h)).await.unwrap();
    assert_eq!(reply(&replies, 1)["result"]["supportedVersions"][0], "2026-07-28");
    assert_eq!(reply(&replies, 2)["result"]["tools"].as_array().unwrap().len(), corelens_mcp::tools::TOOLS.len());
    let call = reply(&replies, 3);
    assert_eq!(call["result"]["isError"], true);
    assert!(call["result"]["content"][0]["text"].as_str().unwrap().contains("is not running"), "{call}");

    // The app listening: the call goes through and comes back.
    let listening = bridge::listen(bridge::endpoint(&dir), Arc::new(Answers)).await.unwrap();
    let h = home.path().to_path_buf();
    let replies = tokio::task::spawn_blocking(move || conversation(&h)).await.unwrap();
    let call = reply(&replies, 3);
    assert_eq!(call["result"]["isError"], false, "{call}");
    assert_eq!(call["result"]["content"][0]["text"], r#"the app ran get_device with {"target":"SW1"}"#);
    listening.stop().await;
}

/// `CORELENS_MCP_DEBUG=1` logs each call's tool and keys to stderr, never a
/// value; without it stderr says nothing about calls.
#[test]
fn the_debug_log_names_tools_and_keys_and_never_values() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(data_dir(home.path())).unwrap();
    let run = |debug: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_corelens-desktop-ai"));
        cmd.arg("--mcp").env("XDG_DATA_HOME", home.path()).env("HOME", home.path()).env("LOCALAPPDATA", home.path()).env_remove("CORELENS_MCP_DEBUG");
        if debug {
            cmd.env("CORELENS_MCP_DEBUG", "1");
        }
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "run_show_command", "arguments": { "target": "value-never-logged", "command": "show value-never-logged" } } })).unwrap();
        drop(stdin);
        let out = child.wait_with_output().unwrap();
        String::from_utf8(out.stderr).unwrap()
    };
    let logged = run(true);
    assert!(logged.contains("tools/call run_show_command: params keys: arguments, name; argument keys: command, target"), "{logged}");
    assert!(!logged.contains("value-never-logged"), "{logged}");
    assert!(!run(false).contains("tools/call"));
}
