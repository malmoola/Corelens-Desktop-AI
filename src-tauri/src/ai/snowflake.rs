//! Snowflake Cortex, streamed, with tools.
//!
//! Cortex's OpenAI-compatible Chat Completions, as Snowflake documents it
//! (checked 2026-11-16, "Cortex REST API" and the REST API's
//! authentication page): `POST https://<account>.snowflakecomputing.com
//! /api/v2/cortex/v1/chat/completions`, the programmatic access token in
//! `Authorization: Bearer` with `X-Snowflake-Authorization-Token-Type:
//! PROGRAMMATIC_ACCESS_TOKEN`. Tools of type `function` are answered by the
//! Claude and OpenAI models Cortex serves; `max_completion_tokens`, not
//! `max_tokens`. A Claude stream ends without a `finish_reason`, which the
//! chat parser already reads as a tool call when calls arrived.
//!
//! The models the account can use: `GET /api/v2/cortex/models`. Its shape
//! is not documented, so it is read leniently, and when it lists nothing
//! the documented tool-capable models are offered instead.
//!
//! The account's role needs the database role `SNOWFLAKE.CORTEX_USER` (or
//! `CORTEX_REST_API_USER`); a 403 says so in words.

use serde_json::{json, Value};

use super::openai::{OpenAi, Style};
use super::provider::*;

/// The models Snowflake documents as calling tools on Chat Completions, the
/// first being the default.
pub const TOOL_MODELS: &[&str] = &[
    "claude-sonnet-5-5",
    "claude-opus-5-5",
    "claude-haiku-5-5",
    "claude-sonnet-4-5",
    "openai-gpt-5.4",
    "openai-gpt-5",
];

pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";

/// `https://<account>.snowflakecomputing.com` from what was typed: an
/// account identifier (`myorg-myaccount`, or a locator with its region),
/// the host, or the whole URL. An underscore becomes a hyphen, as Snowflake
/// asks for in a host name.
pub fn host_for(account: &str) -> Result<String, String> {
    let a = account.trim().to_ascii_lowercase();
    let a = a.strip_prefix("https://").or_else(|| a.strip_prefix("http://")).unwrap_or(&a);
    let a = a.split('/').next().unwrap_or("");
    if a.contains('@') {
        return Err("The Snowflake account must not carry a user name or password; the token is kept in the keychain.".into());
    }
    let a = a.strip_suffix(".snowflakecomputing.com").unwrap_or(a).replace('_', "-");
    if a.is_empty() || a.starts_with(['.', '-']) || a.ends_with(['.', '-']) || !a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') {
        return Err("That is not a Snowflake account identifier. Type it as orgname-accountname (Snowsight → your name → Account → View account details).".into());
    }
    Ok(format!("https://{a}.snowflakecomputing.com"))
}

pub fn models_url(host: &str) -> String {
    format!("{host}/api/v2/cortex/models")
}

pub struct Snowflake {
    inner: OpenAi,
}

impl Snowflake {
    /// `host` from [`host_for`].
    pub fn new(token: zeroize::Zeroizing<String>, model: String, host: &str) -> Self {
        Snowflake { inner: OpenAi { key: token, model, base: format!("{host}/api/v2/cortex/v1"), style: Style::Chat } }
    }

    fn auth(&self) -> Vec<(String, String)> {
        let mut h = self.inner.headers();
        h.push(("x-snowflake-authorization-token-type".into(), "PROGRAMMATIC_ACCESS_TOKEN".into()));
        h
    }

    /// For a GET (the model list): the token, and that JSON is wanted.
    /// Without an `Accept` of its own the HTTP client sends `*/*`, which
    /// Snowflake's `/api/v2` answers with 415 "Invalid input value. null".
    /// A GET has no body, so no `Content-Type`.
    pub fn headers(&self) -> Vec<(String, String)> {
        let mut h = self.auth();
        h.push(("accept".into(), "application/json".into()));
        h
    }
}

impl Provider for Snowflake {
    fn id(&self) -> &'static str {
        "snowflake"
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    fn request(&self, turn: &Turn) -> HttpRequest {
        let mut r = self.inner.request(turn);
        // The stream, or JSON when it is an error, as Snowflake documents.
        r.headers = self.auth();
        r.headers.push(("accept".into(), "application/json, text/event-stream".into()));
        r.body["max_completion_tokens"] = json!(turn.max_tokens);
        // An empty tool list with a tool choice is not a valid request.
        if turn.tools.is_empty() {
            if let Some(o) = r.body.as_object_mut() {
                o.remove("tools");
                o.remove("tool_choice");
            }
        }
        r
    }

    fn parser(&self) -> Box<dyn StreamParser> {
        self.inner.parser()
    }

    fn error(&self, status: u16, retry_after: Option<u64>, body: &str) -> ProviderError {
        let message = error_message(body);
        let lower = message.to_ascii_lowercase();
        match status {
            401 => ProviderError::BadKey(message),
            _ if lower.contains("pat_invalid") || lower.contains("programmatic access token") => ProviderError::BadKey(message),
            403 => ProviderError::BadKey(format!(
                "Snowflake refused the request: the token's user needs a default role granted SNOWFLAKE.CORTEX_USER, and the account must allow the Cortex REST API ({message})"
            )),
            402 => ProviderError::Billing(format!("the account's Cortex budget is used up ({message})")),
            404 => ProviderError::ModelNotFound(format!("{} ({message})", self.inner.model())),
            400 if lower.contains("model") && (lower.contains("unknown") || lower.contains("not available") || lower.contains("not eligible") || lower.contains("invalid")) => {
                ProviderError::ModelNotFound(format!("{} ({message})", self.inner.model()))
            }
            400 if lower.contains("throttl") => ProviderError::RateLimited { retry_after, message },
            429 => ProviderError::RateLimited { retry_after, message },
            500..=599 => ProviderError::Overloaded(message),
            _ => ProviderError::Other(format!("Snowflake answered {status}: {message}")),
        }
    }
}

/// The model names `GET /api/v2/cortex/models` lists, in whatever of the
/// likely shapes it comes: a list of names, or of objects naming the model,
/// on its own or under `models` or `data`.
pub fn models_from(v: &Value) -> Vec<String> {
    let list = v.as_array().or_else(|| v["models"].as_array()).or_else(|| v["data"].as_array());
    let mut ids: Vec<String> = list
        .into_iter()
        .flatten()
        .filter_map(|m| m.as_str().or_else(|| m["name"].as_str()).or_else(|| m["model"].as_str()).or_else(|| m["id"].as_str()).map(str::to_string))
        .filter(|s| !s.trim().is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Whether Snowflake documents the model as calling tools.
pub fn calls_tools(model: &str) -> bool {
    model.starts_with("claude-") || model.starts_with("openai-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn turn() -> Turn {
        Turn { system: "be brief".into(), messages: vec![Message::User { text: "hello".into() }], tools: vec![], max_tokens: 4000 }
    }

    #[test]
    fn an_account_is_typed_any_of_the_ways_snowflake_shows_it() {
        for typed in ["myorg-myaccount", "MYORG-MYACCOUNT", "myorg_myaccount", " https://myorg-myaccount.snowflakecomputing.com/ ", "myorg-myaccount.snowflakecomputing.com/console"] {
            assert_eq!(host_for(typed).unwrap(), "https://myorg-myaccount.snowflakecomputing.com", "{typed}");
        }
        assert_eq!(host_for("xy12345.us-east-2.aws").unwrap(), "https://xy12345.us-east-2.aws.snowflakecomputing.com");
        assert!(host_for("").is_err());
        assert!(host_for("user:pass@myorg-myaccount").unwrap_err().contains("user name"));
        assert!(host_for("my org").is_err());
        // Whatever is typed, the request goes to Snowflake and nowhere else.
        for typed in ["evil.example/../x", "attacker.example#", "a.b.c"] {
            if let Ok(h) = host_for(typed) {
                assert!(h.ends_with(".snowflakecomputing.com") && !h[8..].contains('/'), "{typed} -> {h}");
            }
        }
    }

    #[test]
    fn a_request_is_cortex_chat_completions_with_the_token_type_said() {
        let p = Snowflake::new(zeroize::Zeroizing::new("FIXTURE-pat-not-a-real-token-0000".into()), "claude-sonnet-5-5".into(), "https://myorg-myaccount.snowflakecomputing.com");
        let r = p.request(&turn());
        assert_eq!(r.url, "https://myorg-myaccount.snowflakecomputing.com/api/v2/cortex/v1/chat/completions");
        assert!(r.headers.contains(&("authorization".into(), "Bearer FIXTURE-pat-not-a-real-token-0000".into())));
        assert!(r.headers.contains(&("x-snowflake-authorization-token-type".into(), "PROGRAMMATIC_ACCESS_TOKEN".into())));
        assert_eq!(r.body["model"], "claude-sonnet-5-5");
        assert_eq!(r.body["max_completion_tokens"], 4000);
        assert!(r.body.get("max_tokens").is_none());
        assert_eq!(r.body["stream"], true);
        assert_eq!(r.body["messages"][0]["role"], "system");
        assert!(!r.body.to_string().contains("FIXTURE-pat"), "the token is a header, never the body");
        assert_eq!(p.id(), "snowflake");
    }

    #[test]
    fn a_claude_stream_with_no_finish_reason_still_reads_as_a_tool_call() {
        let p = Snowflake::new(zeroize::Zeroizing::new("FIXTURE-pat-not-a-real-token-0000".into()), "claude-sonnet-5-5".into(), "https://a.snowflakecomputing.com");
        let mut parser = p.parser();
        parser
            .feed(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"t1\",\"type\":\"function\",\"function\":{\"name\":\"get_device\",\"arguments\":\"{\\\"target\\\":\\\"SW1\\\"}\"}}]}}]}\n\ndata: [DONE]\n\n")
            .unwrap();
        let r = parser.finish().unwrap();
        assert_eq!(r.stop, Stop::ToolUse);
        assert_eq!(r.calls[0].name, "get_device");
    }

    /// The calls a stream comes out as: `(name, arguments)` in order.
    fn calls_from(events: &[Value]) -> Vec<(String, Value)> {
        let p = Snowflake::new(zeroize::Zeroizing::new("FIXTURE-pat-not-a-real-token-0000".into()), "claude-sonnet-4-5".into(), "https://a.snowflakecomputing.com");
        let mut parser = p.parser();
        let stream: String = events.iter().map(|e| format!("data: {e}\n\n")).collect::<String>() + "data: [DONE]\n\n";
        // Fed a few bytes at a time, as a network delivers it.
        for piece in stream.as_bytes().chunks(7) {
            parser.feed(piece).unwrap();
        }
        let r = parser.finish().unwrap();
        assert_eq!(r.stop, Stop::ToolUse);
        r.calls.into_iter().map(|c| (c.name, c.arguments)).collect()
    }

    fn chunk(calls: Value) -> Value {
        json!({ "choices": [{ "index": 0, "delta": { "tool_calls": calls } }] })
    }

    /// Streams in the shapes Cortex sends for Claude and OpenAI models,
    /// reconstructed from its documentation and from the stream the chat
    /// was seen to glue together: Claude's chunks repeat the call's id and
    /// name on every fragment, and number every call 0.
    #[test]
    fn streamed_tool_calls_keep_their_names_and_arguments_apart() {
        // One call, its name on every fragment.
        let one = calls_from(&[
            chunk(json!([{ "index": 0, "id": "tooluse_A", "type": "function", "function": { "name": "run_show_command", "arguments": "" } }])),
            chunk(json!([{ "index": 0, "id": "tooluse_A", "type": "function", "function": { "name": "run_show_command", "arguments": "{\"target\":\"acc" } }])),
            chunk(json!([{ "index": 0, "id": "tooluse_A", "type": "function", "function": { "name": "run_show_command", "arguments": "ess\",\"command\":\"show ip arp\"}" } }])),
        ]);
        assert_eq!(one, [("run_show_command".to_string(), json!({ "target": "access", "command": "show ip arp" }))]);

        // Two in parallel, both numbered 0, told apart by their ids.
        let two = calls_from(&[
            chunk(json!([{ "index": 0, "id": "tooluse_A", "type": "function", "function": { "name": "get_device", "arguments": "" } }])),
            chunk(json!([{ "index": 0, "id": "tooluse_A", "function": { "name": "get_device", "arguments": "{\"target\":\"core\"}" } }])),
            chunk(json!([{ "index": 0, "id": "tooluse_B", "type": "function", "function": { "name": "run_show_command", "arguments": "" } }])),
            chunk(json!([{ "index": 0, "id": "tooluse_B", "function": { "name": "run_show_command", "arguments": "{\"target\":\"access\",\"command\":\"show vlan\"}" } }])),
        ]);
        assert_eq!(two, [("get_device".to_string(), json!({ "target": "core" })), ("run_show_command".to_string(), json!({ "target": "access", "command": "show vlan" }))]);

        // The same tool twice, OpenAI's way: numbered, the id and name only
        // on the first fragment of each.
        let twice = calls_from(&[
            chunk(json!([{ "index": 0, "id": "call_1", "type": "function", "function": { "name": "run_show_command", "arguments": "" } }])),
            chunk(json!([{ "index": 0, "function": { "arguments": "{\"target\":\"core\"," } }])),
            chunk(json!([{ "index": 1, "id": "call_2", "type": "function", "function": { "name": "run_show_command", "arguments": "" } }])),
            chunk(json!([{ "index": 0, "function": { "arguments": "\"command\":\"show ip route\"}" } }])),
            chunk(json!([{ "index": 1, "function": { "arguments": "{\"target\":\"access\",\"command\":\"show ip route\"}" } }])),
        ]);
        assert_eq!(
            twice,
            [
                ("run_show_command".to_string(), json!({ "target": "core", "command": "show ip route" })),
                ("run_show_command".to_string(), json!({ "target": "access", "command": "show ip route" }))
            ]
        );

        // Anthropic's events, as Cortex can pass Claude's stream through.
        let anthropic = calls_from(&[
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use", "id": "toolu_1", "name": "get_device", "input": {} } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "{\"target\":" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "\"access-switch\"}" } }),
            json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "tool_use", "id": "toolu_2", "name": "get_device", "input": {} } }),
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "{\"target\":\"core\"}" } }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" } }),
        ]);
        assert_eq!(anthropic, [("get_device".to_string(), json!({ "target": "access-switch" })), ("get_device".to_string(), json!({ "target": "core" }))]);

        // Cortex's own `inference:complete` delta: the id and name repeated
        // on every fragment of the input.
        let native = |id: &str, input: &str| json!({ "choices": [{ "delta": { "type": "tool_use", "tool_use_id": id, "name": "run_show_command", "input": input } }] });
        let cortex = calls_from(&[
            native("tooluse_X", ""),
            native("tooluse_X", "{\"target\":\"access\","),
            native("tooluse_X", "\"command\":\"show clock\"}"),
            native("tooluse_Y", "{\"target\":\"core\",\"command\":\"show clock\"}"),
        ]);
        assert_eq!(
            cortex,
            [
                ("run_show_command".to_string(), json!({ "target": "access", "command": "show clock" })),
                ("run_show_command".to_string(), json!({ "target": "core", "command": "show clock" }))
            ]
        );
    }

    #[test]
    fn errors_are_said_in_words() {
        let p = Snowflake::new(zeroize::Zeroizing::new("FIXTURE-pat-not-a-real-token-0000".into()), "claude-sonnet-5-5".into(), "https://a.snowflakecomputing.com");
        assert!(matches!(p.error(401, None, r#"{"message":"PAT_INVALID"}"#), ProviderError::BadKey(_)));
        assert!(matches!(p.error(403, None, r#"{"message":"Not Authorized"}"#), ProviderError::BadKey(m) if m.contains("CORTEX_USER")));
        assert!(matches!(p.error(402, None, r#"{"message":"budget exceeded"}"#), ProviderError::Billing(_)));
        assert!(matches!(p.error(400, None, r#"{"message":"unknown model \"claude-x\""}"#), ProviderError::ModelNotFound(_)));
        assert!(matches!(p.error(429, Some(4), r#"{"message":"too many requests"}"#), ProviderError::RateLimited { retry_after: Some(4), .. }));
        assert!(matches!(p.error(503, None, r#"{"message":"inference timed out"}"#), ProviderError::Overloaded(_)));
    }

    /// A stand-in for Snowflake's `/api/v2` front door, held to what its
    /// documentation asks of a request: a 415 `Invalid input value. null`
    /// for a request that does not say it accepts JSON (the stream as well,
    /// on a POST) or posts anything but `application/json`. Keeps every
    /// request head it was sent.
    async fn picky_snowflake() -> (String, Arc<Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let heads = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&heads);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let head = loop {
                    let Ok(k) = sock.read(&mut chunk).await else { break String::new() };
                    if k == 0 {
                        break String::new();
                    }
                    buf.extend_from_slice(&chunk[..k]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break text[..end].to_string();
                        }
                    }
                };
                seen.lock().unwrap().push(head.clone());
                let lower = head.to_ascii_lowercase();
                let header = |name: &str| lower.lines().find_map(|l| l.strip_prefix(&format!("{name}:")).map(|v| v.trim().to_string()));
                let accept = header("accept").unwrap_or_default();
                let post = lower.starts_with("post ");
                let ok = accept.contains("application/json") && (!post || (accept.contains("text/event-stream") && header("content-type").as_deref() == Some("application/json")));
                let reply = if !ok {
                    "HTTP/1.1 415 Unsupported Media Type\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"message\":\"Invalid input value. null\"}".to_string()
                } else if post {
                    format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\ndata: {}\n\ndata: [DONE]\n\n", json!({ "choices": [{ "index": 0, "delta": { "content": "Hello." }, "finish_reason": "stop" }] }))
                } else {
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{\"models\":[{\"name\":\"claude-sonnet-4-5\"}]}".to_string()
                };
                let _ = sock.write_all(reply.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        (base, heads)
    }

    #[tokio::test]
    async fn every_call_says_what_it_accepts_so_snowflake_takes_it() {
        let (base, heads) = picky_snowflake().await;
        let p = Snowflake::new(zeroize::Zeroizing::new("FIXTURE-pat-not-a-real-token-0000".into()), "claude-sonnet-4-5".into(), &base);
        // Refresh models and Test connection.
        let listed = get(&p, &models_url(&base), &p.headers()).await.map_err(|e| e.to_string());
        assert_eq!(listed.map(|v| models_from(&v)), Ok(vec!["claude-sonnet-4-5".to_string()]), "the model list; heads: {:#?}", heads.lock().unwrap());
        // A chat turn.
        let said = send(&p, &turn(), &crate::mcp::redact::Secrets::default(), &|_| {}, &|_| {}).await.map_err(|e| e.to_string());
        assert_eq!(said.map(|r| r.text), Ok("Hello.".to_string()), "a turn; heads: {:#?}", heads.lock().unwrap());
        let heads = heads.lock().unwrap();
        assert!(heads[0].starts_with("GET ") && !heads[0].to_ascii_lowercase().contains("content-type"), "a GET carries no body and no content type: {}", heads[0]);
    }

    /// Against a real account, by hand only: the model list, then "hi"
    /// on Chat Completions exactly as the chat sends it, then "hi" on
    /// `inference:complete` for comparison. Prints each call's status and
    /// answer as Show last request records them — the token never.
    ///
    /// ```text
    /// CORELENS_SNOWFLAKE_ACCOUNT=orgname-accountname [CORELENS_SNOWFLAKE_MODEL=claude-sonnet-4-5] \
    ///   cargo test -p corelens-desktop-ai --bin corelens-desktop-ai snowflake_live -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "calls a real Snowflake account with the token kept in this machine's keychain"]
    async fn snowflake_live_says_hi_with_the_stored_token() {
        let account = std::env::var("CORELENS_SNOWFLAKE_ACCOUNT").expect("set CORELENS_SNOWFLAKE_ACCOUNT to the account identifier (orgname-accountname)");
        let model = std::env::var("CORELENS_SNOWFLAKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".into());
        let host = host_for(&account).expect("an account identifier");
        let token = crate::ai::keys::read("snowflake").expect("the keychain").expect("no Snowflake token is kept: paste one in Settings → AI first");
        let shown = |what: &str| {
            let e = last_exchange("snowflake").unwrap_or_default();
            println!("\n=== {what}\n{}", serde_json::to_string_pretty(&e).unwrap_or_default());
        };
        println!("host: {host}  model: {model}");

        let p = Snowflake::new(token.clone(), model.clone(), &host);
        let listed = get(&p, &models_url(&host), &p.headers()).await;
        shown("GET /api/v2/cortex/models");
        println!("-> {}", match &listed { Ok(v) => format!("{} model(s) read", models_from(v).len()), Err(e) => e.to_string() });

        let t = Turn { system: "Answer in one short sentence.".into(), messages: vec![Message::User { text: "hi".into() }], tools: vec![], max_tokens: 64 };
        let said = send(&p, &t, &crate::mcp::redact::Secrets::default(), &|_| {}, &|_| {}).await;
        shown("POST /api/v2/cortex/v1/chat/completions (what the chat sends)");
        println!("-> {}", match &said { Ok(r) => format!("answered: {:?}", r.text), Err(e) => e.to_string() });

        let http = client().expect("an HTTP client");
        let url = format!("{host}/api/v2/cortex/inference:complete");
        let r = http
            .post(&url)
            .header("authorization", format!("Bearer {}", token.as_str()))
            .header("x-snowflake-authorization-token-type", "PROGRAMMATIC_ACCESS_TOKEN")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(json!({ "model": model, "messages": [{ "role": "user", "content": "hi" }], "stream": true }).to_string())
            .send()
            .await;
        println!("\n=== POST {url} (for comparison)");
        match r {
            Ok(r) => {
                let status = r.status().as_u16();
                let text: String = r.text().await.unwrap_or_default().chars().take(2000).collect();
                println!("status: {status}\n{}", crate::mcp::redact::shapes_only(&text.replace(token.as_str(), "<redacted>")));
            }
            Err(e) => println!("no answer: {}", clean_error(&e)),
        }
        assert!(said.is_ok(), "the chat's own request failed; see above");
    }

    #[test]
    fn the_model_list_is_read_in_any_likely_shape() {
        assert_eq!(models_from(&json!(["mistral-large2", "claude-sonnet-5-5"])), ["claude-sonnet-5-5", "mistral-large2"]);
        assert_eq!(models_from(&json!({ "models": [{ "name": "claude-opus-5-5" }, { "name": "llama3.3-70b" }] })), ["claude-opus-5-5", "llama3.3-70b"]);
        assert_eq!(models_from(&json!({ "data": [{ "id": "openai-gpt-5" }] })), ["openai-gpt-5"]);
        assert!(models_from(&json!({ "something": "else" })).is_empty());
        assert!(calls_tools("claude-sonnet-5-5") && calls_tools("openai-gpt-5") && !calls_tools("llama3.3-70b"));
    }
}
