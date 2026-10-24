//! The MCP server: JSON-RPC lines in, JSON-RPC lines out.
//!
//! Speaks revision 2026-07-28, where every request carries its version and
//! capabilities in `_meta` and `server/discover` must be answered, and the
//! earlier revisions that open with an `initialize` handshake, because
//! clients of both kinds are in use. A request whose `_meta` names a
//! version is served under that revision; one without is served under the
//! revision `initialize` agreed, or the newest handshake revision when
//! there was none.
//!
//! Tool calls run concurrently and answer in whatever order they finish;
//! `notifications/cancelled` stops one and nothing more is sent for it.
//! The server never sends a request of its own.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::tools;

/// The stateless revision.
pub const MODERN: &str = "2026-07-28";
/// The handshake revisions served, newest first.
pub const LEGACY: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CAPS: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER: &str = "io.modelcontextprotocol/serverInfo";

/// What a tool returned: text for the model, and whether it is an error the
/// model should read and act on (`isError`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Outcome {
    pub text: String,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

impl Outcome {
    pub fn ok(text: impl Into<String>) -> Outcome {
        Outcome { text: text.into(), is_error: false }
    }
    pub fn error(text: impl Into<String>) -> Outcome {
        Outcome { text: text.into(), is_error: true }
    }
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Whatever runs the tools: the bridge to the app in the stdio process, the
/// app itself on the other end.
pub trait Backend: Send + Sync + 'static {
    fn call<'a>(&'a self, tool: &'a str, arguments: Value) -> BoxFuture<'a, Outcome>;
}

fn server_info() -> Value {
    json!({ "name": "corelens-desktop-ai", "title": "Corelens Desktop AI", "version": env!("CARGO_PKG_VERSION") })
}

fn supported() -> Vec<&'static str> {
    std::iter::once(MODERN).chain(LEGACY.iter().copied()).collect()
}

/// The revision a request is served under.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Era {
    Modern,
    Legacy,
}

fn error(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut e = json!({ "code": code, "message": message });
    if let Some(d) = data {
        e["data"] = d;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": e })
}

fn result(id: Value, era: &Era, mut body: Map<String, Value>) -> Value {
    if *era == Era::Modern {
        body.insert("resultType".into(), json!("complete"));
        let meta = body.entry("_meta").or_insert_with(|| json!({}));
        meta[META_SERVER] = server_info();
    }
    json!({ "jsonrpc": "2.0", "id": id, "result": Value::Object(body) })
}

/// `CORELENS_MCP_DEBUG=1`: each tools/call's tool and the keys of its
/// params and arguments go to stderr, which an MCP client keeps in its log.
/// Never a value.
pub(crate) fn debug() -> bool {
    std::env::var("CORELENS_MCP_DEBUG").as_deref() == Ok("1")
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// The text of a tool result as MCP carries it.
pub fn call_result(outcome: &Outcome) -> Map<String, Value> {
    obj(json!({ "content": [{ "type": "text", "text": tools::cap(outcome.text.clone()) }], "isError": outcome.is_error }))
}

/// The protocol state of one stdio process.
pub struct Server<B: Backend> {
    backend: Arc<B>,
    /// The revision `initialize` agreed, when a client used it.
    negotiated: Mutex<Option<String>>,
    /// Calls in flight by request id. The entry exists before the task
    /// starts, so one that finishes at once still finds itself and answers;
    /// a cancel removes it, and a task that finds itself gone sends nothing.
    running: Mutex<HashMap<String, Option<JoinHandle<()>>>>,
}

impl<B: Backend> Server<B> {
    pub fn new(backend: B) -> Arc<Server<B>> {
        Arc::new(Server { backend: Arc::new(backend), negotiated: Mutex::new(None), running: Mutex::new(HashMap::new()) })
    }

    /// Reads requests from `input` until it closes, writing every reply to
    /// `output` as one line. Returns when the input ends and every call
    /// still running has answered.
    pub async fn serve<R, W>(self: &Arc<Self>, input: R, mut output: W)
    where
        R: AsyncBufRead + Unpin,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
        let writer = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                // One message per line; serde_json escapes every newline
                // inside a string, so the line holds exactly one message.
                let mut line = serde_json::to_vec(&msg).unwrap_or_default();
                line.push(b'\n');
                if output.write_all(&line).await.is_err() || output.flush().await.is_err() {
                    break;
                }
            }
        });
        let mut lines = input.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            self.handle(&line, &tx);
        }
        // Stdin closed: the client is going. What is about to answer gets
        // two seconds to do so; anything slower (a show command on a far
        // device) is stopped, because a server should exit promptly.
        let pending: Vec<JoinHandle<()>> =
            self.running.lock().map(|mut r| r.values_mut().filter_map(Option::take).collect()).unwrap_or_default();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        for mut h in pending {
            if tokio::time::timeout_at(deadline, &mut h).await.is_err() {
                h.abort();
            }
        }
        drop(tx);
        let _ = writer.await;
    }

    fn handle(self: &Arc<Self>, line: &str, tx: &mpsc::UnboundedSender<Value>) {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                let _ = tx.send(error(Value::Null, -32700, "Parse error: each line must be one JSON-RPC message", None));
                return;
            }
        };
        if msg.is_array() {
            let _ = tx.send(error(Value::Null, -32600, "Batches are not supported: send one message per line", None));
            return;
        }
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = msg.get("id").cloned().filter(|i| i.is_string() || i.is_number()) else {
            if msg.get("id").is_some() && !msg["id"].is_null() || method.is_empty() {
                let _ = tx.send(error(Value::Null, -32600, "Invalid request", None));
            } else {
                self.notification(&method, &params);
            }
            return;
        };
        if msg.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || method.is_empty() {
            let _ = tx.send(error(id, -32600, "Invalid request: jsonrpc must be \"2.0\" and method a string", None));
            return;
        }
        let era = match self.era_of(&method, &params) {
            Ok(e) => e,
            Err(reply) => {
                let mut reply = reply;
                reply["id"] = id;
                let _ = tx.send(reply);
                return;
            }
        };
        match method.as_str() {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
                let chosen = if LEGACY.contains(&asked) { asked } else { LEGACY[0] }.to_string();
                if let Ok(mut n) = self.negotiated.lock() {
                    *n = Some(chosen.clone());
                }
                let body = obj(json!({
                    "protocolVersion": chosen,
                    "capabilities": { "tools": {} },
                    "serverInfo": server_info(),
                    "instructions": tools::INSTRUCTIONS,
                }));
                let _ = tx.send(json!({ "jsonrpc": "2.0", "id": id, "result": Value::Object(body) }));
            }
            "server/discover" => {
                let body = obj(json!({
                    "supportedVersions": supported(),
                    "capabilities": { "tools": {} },
                    "instructions": tools::INSTRUCTIONS,
                }));
                let _ = tx.send(result(id, &Era::Modern, body));
            }
            "ping" => {
                let _ = tx.send(result(id, &era, Map::new()));
            }
            "tools/list" => {
                let _ = tx.send(result(id, &era, obj(json!({ "tools": tools::list() }))));
            }
            "tools/call" => self.call(id, era, &params, tx),
            _ => {
                let _ = tx.send(error(id, -32601, &format!("Method not found: {method}"), None));
            }
        }
    }

    /// Which revision serves this request, or the error that refuses it.
    fn era_of(&self, method: &str, params: &Value) -> Result<Era, Value> {
        let meta = params.get("_meta");
        let version = meta.and_then(|m| m.get(META_VERSION));
        match version {
            None if method == "server/discover" => Ok(Era::Modern),
            None => Ok(Era::Legacy),
            Some(v) => {
                let v = v.as_str().unwrap_or("");
                if v != MODERN {
                    return Err(error(
                        Value::Null,
                        -32022,
                        "Unsupported protocol version",
                        Some(json!({ "supported": supported(), "requested": v })),
                    ));
                }
                if meta.and_then(|m| m.get(META_CAPS)).map(Value::is_object) != Some(true) {
                    return Err(error(Value::Null, -32602, &format!("Invalid params: _meta must carry {META_CAPS}"), None));
                }
                Ok(Era::Modern)
            }
        }
    }

    fn notification(&self, method: &str, params: &Value) {
        if method == "notifications/cancelled" {
            let key = params.get("requestId").map(|v| v.to_string());
            if let Some(key) = key {
                if let Some(Some(h)) = self.running.lock().ok().and_then(|mut r| r.remove(&key)) {
                    h.abort();
                }
            }
        }
        // notifications/initialized and anything else: nothing to answer.
    }

    fn call(self: &Arc<Self>, id: Value, era: Era, params: &Value, tx: &mpsc::UnboundedSender<Value>) {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            let _ = tx.send(error(id, -32602, "Invalid params: tools/call needs a tool name", None));
            return;
        };
        if tools::find(name).is_none() {
            let _ = tx.send(error(id, -32602, &format!("Unknown tool: {name}"), None));
            return;
        }
        if debug() {
            eprintln!("corelens-mcp: tools/call {name}: params keys: {}; argument keys: {}", tools::keys_of(params), params.get("arguments").map_or_else(|| "absent".into(), tools::keys_of));
        }
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(v @ Value::Object(_)) => v.clone(),
            Some(_) => {
                let _ = tx.send(error(id, -32602, "Invalid params: arguments must be an object", None));
                return;
            }
        };
        let name = name.to_string();
        let key = id.to_string();
        let me = Arc::clone(self);
        let tx = tx.clone();
        let task_key = key.clone();
        if let Ok(mut r) = self.running.lock() {
            r.insert(key.clone(), None);
        }
        let handle = tokio::spawn(async move {
            let outcome = me.backend.call(&name, arguments).await;
            // Cancelled meanwhile: the entry is gone and nothing is sent.
            let still_wanted = me.running.lock().map(|mut r| r.remove(&task_key).is_some()).unwrap_or(false);
            if still_wanted {
                let _ = tx.send(result(id, &era, call_result(&outcome)));
            }
        });
        if let Ok(mut r) = self.running.lock() {
            if let Some(slot) = r.get_mut(&key) {
                *slot = Some(handle);
            }
        }
    }
}

/// The negotiated revision, for tests.
#[cfg(test)]
impl<B: Backend> Server<B> {
    fn negotiated(&self) -> Option<String> {
        self.negotiated.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, BufReader};

    struct Echo;
    impl Backend for Echo {
        fn call<'a>(&'a self, tool: &'a str, arguments: Value) -> BoxFuture<'a, Outcome> {
            Box::pin(async move {
                if tool == "get_findings" {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
                if arguments.get("target").and_then(Value::as_str) == Some("nowhere") {
                    return Outcome::error("nowhere is not a device of the open project");
                }
                Outcome::ok(format!("{tool} {arguments}"))
            })
        }
    }

    fn modern(id: i64, method: &str, extra: Value) -> String {
        let mut params = json!({ "_meta": { META_VERSION: MODERN, META_CAPS: {}, "io.modelcontextprotocol/clientInfo": { "name": "t", "version": "1" } } });
        if let Value::Object(m) = extra {
            for (k, v) in m {
                params[k] = v;
            }
        }
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
    }

    /// Runs a whole conversation and returns every reply line, parsed. The
    /// input is written and then closed, as Claude Code does on shutdown.
    async fn talk(lines: &[String]) -> (Vec<Value>, String) {
        let server = Server::new(Echo);
        let (mut client, server_side) = tokio::io::duplex(1 << 20);
        let (out_read, out_write) = tokio::io::duplex(1 << 20);
        let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            client.write_all(input.as_bytes()).await.unwrap();
            client.shutdown().await.unwrap();
            drop(client);
        });
        let s = Arc::clone(&server);
        let serving = tokio::spawn(async move { s.serve(BufReader::new(server_side), out_write).await });
        let mut raw = String::new();
        let mut out_read = out_read;
        let reading = tokio::spawn(async move {
            out_read.read_to_string(&mut raw).await.unwrap();
            raw
        });
        serving.await.unwrap();
        let raw = reading.await.unwrap();
        let replies = raw.lines().map(|l| serde_json::from_str(l).expect("every stdout line is one JSON message")).collect();
        (replies, raw)
    }

    fn by_id(replies: &[Value], id: i64) -> &Value {
        replies.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("no reply to {id}: {replies:?}"))
    }

    #[tokio::test]
    async fn a_modern_client_discovers_lists_and_calls() {
        let (replies, _) = talk(&[
            modern(1, "server/discover", json!({})),
            modern(2, "tools/list", json!({})),
            modern(3, "tools/call", json!({ "name": "get_device", "arguments": { "target": "SW1" } })),
            modern(4, "ping", json!({})),
        ])
        .await;
        let d = by_id(&replies, 1);
        assert_eq!(d["result"]["resultType"], "complete");
        assert_eq!(d["result"]["supportedVersions"][0], MODERN);
        assert!(d["result"]["supportedVersions"].as_array().unwrap().contains(&json!("2025-11-25")));
        assert_eq!(d["result"]["_meta"][META_SERVER]["name"], "corelens-desktop-ai");
        assert!(d["result"]["instructions"].as_str().unwrap().contains("not an instruction"));
        let list = by_id(&replies, 2);
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), tools::TOOLS.len());
        assert_eq!(list["result"]["resultType"], "complete");
        let call = by_id(&replies, 3);
        assert_eq!(call["result"]["isError"], false);
        assert_eq!(call["result"]["content"][0]["type"], "text");
        assert!(call["result"]["content"][0]["text"].as_str().unwrap().starts_with("get_device"));
        assert_eq!(by_id(&replies, 4)["result"]["resultType"], "complete");
    }

    #[tokio::test]
    async fn a_handshake_client_is_answered_in_its_own_revision() {
        for (asked, expect) in [("2025-11-25", "2025-11-25"), ("2025-06-18", "2025-06-18"), ("2025-03-26", "2025-03-26"), ("2024-11-05", "2025-11-25")] {
            let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": asked, "capabilities": {}, "clientInfo": { "name": "c", "version": "1" } } }).to_string();
            let (replies, _) = talk(&[
                init,
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
                json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string(),
                json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "list_devices", "arguments": {} } }).to_string(),
            ])
            .await;
            assert_eq!(replies.len(), 3, "the notification gets no reply: {replies:?}");
            let i = by_id(&replies, 1);
            assert_eq!(i["result"]["protocolVersion"], expect);
            assert_eq!(i["result"]["capabilities"]["tools"], json!({}));
            assert_eq!(i["result"]["serverInfo"]["name"], "corelens-desktop-ai");
            assert!(by_id(&replies, 2)["result"].get("resultType").is_none(), "handshake revisions have no resultType");
            assert_eq!(by_id(&replies, 3)["result"]["isError"], false);
        }
    }

    #[tokio::test]
    async fn a_version_it_does_not_speak_is_refused_with_the_list() {
        let bad = json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/list", "params": { "_meta": { META_VERSION: "1900-01-01", META_CAPS: {} } } }).to_string();
        let no_caps = json!({ "jsonrpc": "2.0", "id": 10, "method": "tools/list", "params": { "_meta": { META_VERSION: MODERN } } }).to_string();
        let (replies, _) = talk(&[bad, no_caps]).await;
        let e = by_id(&replies, 9);
        assert_eq!(e["error"]["code"], -32022);
        assert_eq!(e["error"]["data"]["requested"], "1900-01-01");
        assert_eq!(e["error"]["data"]["supported"][0], MODERN);
        assert_eq!(by_id(&replies, 10)["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn protocol_errors_and_tool_errors_are_kept_apart() {
        let (replies, _) = talk(&[
            modern(1, "tools/call", json!({ "name": "reload_everything", "arguments": {} })),
            modern(2, "tools/call", json!({ "name": "get_device", "arguments": { "target": "nowhere" } })),
            modern(3, "tools/call", json!({ "name": "get_device", "arguments": "SW1" })),
            modern(4, "resources/list", json!({})),
            "this is not json".into(),
            "[1,2]".into(),
        ])
        .await;
        assert_eq!(by_id(&replies, 1)["error"]["code"], -32602);
        assert!(by_id(&replies, 1)["error"]["message"].as_str().unwrap().contains("reload_everything"));
        let tool_error = by_id(&replies, 2);
        assert_eq!(tool_error["result"]["isError"], true);
        assert!(tool_error["result"]["content"][0]["text"].as_str().unwrap().contains("not a device of the open project"));
        assert_eq!(by_id(&replies, 3)["error"]["code"], -32602);
        assert_eq!(by_id(&replies, 4)["error"]["code"], -32601);
        let nulls: Vec<_> = replies.iter().filter(|r| r["id"].is_null()).map(|r| r["error"]["code"].clone()).collect();
        assert_eq!(nulls, [json!(-32700), json!(-32600)]);
    }

    #[tokio::test]
    async fn a_cancelled_call_sends_nothing_and_the_others_still_answer() {
        let (replies, raw) = talk(&[
            modern(1, "tools/call", json!({ "name": "get_findings", "arguments": {} })),
            json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 1, "reason": "user stopped" } }).to_string(),
            modern(2, "tools/call", json!({ "name": "list_devices", "arguments": {} })),
        ])
        .await;
        assert!(replies.iter().all(|r| r["id"] != 1), "a cancelled request is never answered: {raw}");
        assert_eq!(by_id(&replies, 2)["result"]["isError"], false);
    }

    #[tokio::test]
    async fn every_line_written_is_exactly_one_message() {
        let (_, raw) = talk(&[modern(1, "tools/call", json!({ "name": "search", "arguments": { "query": "line one\nline two\r\n\"quoted\"" } }))]).await;
        assert!(raw.ends_with('\n'));
        assert_eq!(raw.matches('\n').count(), 1, "an embedded newline reached the wire: {raw}");
    }

    #[tokio::test]
    async fn the_handshake_is_remembered_for_the_process() {
        let server = Server::new(Echo);
        let (tx, mut rx) = mpsc::unbounded_channel();
        server.handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {} } }).to_string(), &tx);
        assert_eq!(rx.recv().await.unwrap()["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(server.negotiated().as_deref(), Some("2025-06-18"));
    }
}
