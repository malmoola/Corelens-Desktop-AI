//! Anthropic's Messages API, streamed, with tools.
//!
//! Written against the API as documented for `anthropic-version:
//! 2026-10-21` (checked 2026-10-25): `POST /v1/messages` with `stream:
//! true`, the key in `x-api-key`; server-sent events `message_start`,
//! `content_block_start` / `_delta` / `_stop`, `message_delta` (the stop
//! reason and the output tokens), `message_stop`, `ping`, `error`. No
//! sampling parameters (that version refuses them). Tools carry
//! `eager_input_streaming`, so their arguments arrive as they are written
//! and are checked here before anything runs. The assistant's content
//! blocks — thinking blocks and their signatures included — are kept and
//! sent back exactly as they came, as the API asks. On the models that
//! have it, the server-side fallback is on (`fallbacks: "default"`), so a
//! request one model declines is answered by another rather than stopping.

use serde_json::{json, Value};

use super::provider::*;
use crate::mcp::redact;

pub const API: &str = "https://api.anthropic.com";
pub const VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// The models the documented server-side fallback applies to.
fn has_fallback(model: &str) -> bool {
    ["claude-fable-5-1", "claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5"].contains(&model)
}

pub struct Anthropic {
    pub key: zeroize::Zeroizing<String>,
    pub model: String,
    /// `API`, or a test's own server.
    pub base: String,
}

impl Anthropic {
    pub fn new(key: zeroize::Zeroizing<String>, model: String) -> Anthropic {
        Anthropic { key, model, base: API.into() }
    }

    pub fn headers(&self) -> Vec<(String, String)> {
        vec![("x-api-key".into(), self.key.to_string()), ("anthropic-version".into(), VERSION.into())]
    }
}

fn messages(turn: &Turn) -> Vec<Value> {
    let mut out = Vec::new();
    for m in &turn.messages {
        match m {
            Message::User { text } => out.push(json!({ "role": "user", "content": text })),
            Message::Assistant { text, calls, native, provider } => {
                let content = match native {
                    // Its own blocks, unchanged, when this provider wrote them.
                    Some(Value::Array(blocks)) if provider == "anthropic" => Value::Array(blocks.clone()),
                    _ => {
                        let mut blocks = Vec::new();
                        if !text.is_empty() {
                            blocks.push(json!({ "type": "text", "text": text }));
                        }
                        for c in calls {
                            blocks.push(json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": c.arguments }));
                        }
                        Value::Array(blocks)
                    }
                };
                out.push(json!({ "role": "assistant", "content": content }));
            }
            // Every result of a turn in one message.
            Message::Tools { results } => out.push(json!({
                "role": "user",
                "content": results.iter().map(|r| json!({ "type": "tool_result", "tool_use_id": r.id, "content": r.content, "is_error": r.is_error })).collect::<Vec<_>>()
            })),
        }
    }
    out
}

impl Provider for Anthropic {
    fn id(&self) -> &'static str {
        "anthropic"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn request(&self, turn: &Turn) -> HttpRequest {
        let tools: Vec<Value> = turn.tools.iter().map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.schema, "eager_input_streaming": true })).collect();
        let mut body = json!({
            "model": self.model,
            "max_tokens": turn.max_tokens,
            "stream": true,
            "system": turn.system,
            "messages": messages(turn),
            "tools": tools,
            "tool_choice": { "type": "auto" },
        });
        let mut headers = self.headers();
        if has_fallback(&self.model) {
            body["fallbacks"] = json!("default");
            headers.push(("anthropic-beta".into(), FALLBACK_BETA.into()));
        }
        HttpRequest { url: format!("{}/v1/messages", self.base), headers, body }
    }

    fn parser(&self) -> Box<dyn StreamParser> {
        Box::new(Parser::default())
    }

    fn error(&self, status: u16, retry_after: Option<u64>, body: &str) -> ProviderError {
        let kind = serde_json::from_str::<Value>(body).ok().and_then(|v| v["error"]["type"].as_str().map(str::to_string)).unwrap_or_default();
        let message = error_message(body);
        match (status, kind.as_str()) {
            (401, _) | (_, "authentication_error") => ProviderError::BadKey(message),
            (402, _) | (_, "billing_error") => ProviderError::Billing(message),
            (_, _) if message.to_ascii_lowercase().contains("credit balance") => ProviderError::Billing(message),
            (403, _) | (_, "permission_error") => ProviderError::Other(format!("This key is not allowed to do that: {message}")),
            (404, _) | (_, "not_found_error") => ProviderError::ModelNotFound(format!("{} ({message})", self.model)),
            (429, _) | (_, "rate_limit_error") => ProviderError::RateLimited { retry_after, message },
            (529, _) | (500, _) | (_, "overloaded_error") | (_, "api_error") => ProviderError::Overloaded(message),
            _ => ProviderError::Other(format!("Anthropic answered {status}: {message}")),
        }
    }
}

/// The blocks of one message as they stream in.
#[derive(Default)]
struct Parser {
    sse: Sse,
    blocks: Vec<Value>,
    /// Tool arguments as they arrive, by block index.
    partial: std::collections::HashMap<usize, String>,
    usage: Usage,
    stop: Option<String>,
    refusal: Option<String>,
    failed: Option<ProviderError>,
}

impl StreamParser for Parser {
    fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Delta>, ProviderError> {
        let mut out = Vec::new();
        for (_, data) in self.sse.feed(chunk) {
            let Ok(ev) = serde_json::from_str::<Value>(&data) else { continue };
            match ev["type"].as_str().unwrap_or("") {
                "message_start" => {
                    let u = &ev["message"]["usage"];
                    self.usage.input_tokens = u["input_tokens"].as_u64().unwrap_or(0) + u["cache_read_input_tokens"].as_u64().unwrap_or(0) + u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                }
                "content_block_start" => {
                    let i = ev["index"].as_u64().unwrap_or(self.blocks.len() as u64) as usize;
                    let block = ev["content_block"].clone();
                    if block["type"] == "tool_use" {
                        out.push(Delta::CallStarted(block["name"].as_str().unwrap_or("").to_string()));
                        self.partial.insert(i, String::new());
                    }
                    while self.blocks.len() <= i {
                        self.blocks.push(Value::Null);
                    }
                    self.blocks[i] = block;
                }
                "content_block_delta" => {
                    let i = ev["index"].as_u64().unwrap_or(0) as usize;
                    let d = &ev["delta"];
                    let Some(block) = self.blocks.get_mut(i) else { continue };
                    match d["type"].as_str().unwrap_or("") {
                        "text_delta" => {
                            let t = d["text"].as_str().unwrap_or("");
                            let now = format!("{}{t}", block["text"].as_str().unwrap_or(""));
                            block["text"] = Value::String(now);
                            out.push(Delta::Text(t.to_string()));
                        }
                        "input_json_delta" => {
                            if let Some(p) = self.partial.get_mut(&i) {
                                p.push_str(d["partial_json"].as_str().unwrap_or(""));
                            }
                        }
                        "thinking_delta" => {
                            let now = format!("{}{}", block["thinking"].as_str().unwrap_or(""), d["thinking"].as_str().unwrap_or(""));
                            block["thinking"] = Value::String(now);
                        }
                        "signature_delta" => {
                            let now = format!("{}{}", block["signature"].as_str().unwrap_or(""), d["signature"].as_str().unwrap_or(""));
                            block["signature"] = Value::String(now);
                        }
                        _ => {}
                    }
                }
                "content_block_stop" => {
                    let i = ev["index"].as_u64().unwrap_or(0) as usize;
                    if let (Some(json), Some(block)) = (self.partial.remove(&i), self.blocks.get_mut(i)) {
                        // An argument stream that is not JSON (cut off by
                        // max_tokens) is left as invalid and refused later.
                        block["input"] = if json.trim().is_empty() { json!({}) } else { serde_json::from_str(&json).unwrap_or_else(|_| json!({ "__invalid_json": true })) };
                    }
                }
                "message_delta" => {
                    if let Some(s) = ev["delta"]["stop_reason"].as_str() {
                        self.stop = Some(s.to_string());
                    }
                    if let Some(d) = ev["delta"]["stop_details"].as_object() {
                        self.refusal = d.get("explanation").and_then(Value::as_str).map(str::to_string).or_else(|| d.get("category").and_then(Value::as_str).map(str::to_string));
                    }
                    if let Some(n) = ev["usage"]["output_tokens"].as_u64() {
                        self.usage.output_tokens = n;
                    }
                }
                "error" => {
                    let t = ev["error"]["type"].as_str().unwrap_or("");
                    let m = redact::shapes_only(ev["error"]["message"].as_str().unwrap_or("the stream ended with an error"));
                    self.failed = Some(match t {
                        "overloaded_error" | "api_error" => ProviderError::Overloaded(m),
                        "rate_limit_error" => ProviderError::RateLimited { retry_after: None, message: m },
                        _ => ProviderError::Other(m),
                    });
                }
                _ => {}
            }
        }
        Ok(out)
    }

    fn finish(self: Box<Self>) -> Result<TurnResult, ProviderError> {
        if let Some(e) = self.failed {
            return Err(e);
        }
        let blocks: Vec<Value> = self.blocks.into_iter().filter(|b| !b.is_null()).collect();
        let mut text = String::new();
        let mut calls = Vec::new();
        for b in &blocks {
            match b["type"].as_str().unwrap_or("") {
                "text" => text.push_str(b["text"].as_str().unwrap_or("")),
                "tool_use" => calls.push(ToolCall { id: b["id"].as_str().unwrap_or("").into(), name: b["name"].as_str().unwrap_or("").into(), arguments: b["input"].clone() }),
                _ => {}
            }
        }
        let stop = match self.stop.as_deref() {
            Some("tool_use") => Stop::ToolUse,
            Some("max_tokens") => Stop::MaxTokens,
            Some("refusal") => Stop::Refused(self.refusal.unwrap_or_else(|| "declined by the model's safeguards".into())),
            Some("end_turn") | Some("stop_sequence") | None => if calls.is_empty() { Stop::EndTurn } else { Stop::ToolUse },
            Some(other) => Stop::Other(other.to_string()),
        };
        Ok(TurnResult { text, calls, native: Some(Value::Array(blocks)), usage: self.usage, stop })
    }
}

/// The model ids `GET /v1/models` lists, newest first as it returns them.
pub fn models_from(v: &Value) -> Vec<(String, String)> {
    v["data"].as_array().map(|a| a.iter().filter_map(|m| Some((m["id"].as_str()?.to_string(), m["display_name"].as_str().unwrap_or("").to_string()))).collect()).unwrap_or_default()
}

pub fn models_url(base: &str) -> String {
    format!("{base}/v1/models?limit=100")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn() -> Turn {
        Turn {
            system: "sys".into(),
            messages: vec![
                Message::User { text: "hi".into() },
                Message::Assistant { text: "".into(), calls: vec![ToolCall { id: "toolu_1".into(), name: "get_device".into(), arguments: json!({ "target": "SW1" }) }], native: None, provider: "openai".into() },
                Message::Tools { results: vec![ToolResult { id: "toolu_1".into(), name: "get_device".into(), content: "ok".into(), is_error: false }] },
            ],
            tools: vec![ToolDef { name: "get_device".into(), description: "d".into(), schema: json!({ "type": "object" }) }],
            max_tokens: 16000,
        }
    }

    #[test]
    fn the_request_is_the_documented_shape() {
        let p = Anthropic::new(zeroize::Zeroizing::new("sk-ant-fixture".into()), "claude-opus-5-5".into());
        let r = p.request(&turn());
        assert_eq!(r.url, "https://api.anthropic.com/v1/messages");
        assert!(r.headers.contains(&("anthropic-version".into(), "2026-10-21".into())));
        assert!(r.headers.contains(&("anthropic-beta".into(), FALLBACK_BETA.into())));
        assert_eq!(r.body["fallbacks"], "default");
        assert_eq!(r.body["stream"], true);
        assert!(r.body.get("temperature").is_none() && r.body.get("top_p").is_none());
        assert_eq!(r.body["tools"][0]["eager_input_streaming"], true);
        assert_eq!(r.body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(r.body["messages"][2]["content"][0]["tool_use_id"], "toolu_1");
        assert!(!r.body.to_string().contains("sk-ant-fixture"), "the key is a header, never the body");
        let haiku = Anthropic::new(zeroize::Zeroizing::new("k".into()), "claude-haiku-5-5".into()).request(&turn());
        assert!(haiku.body.get("fallbacks").is_none(), "no fallback where the model has none");
    }

    #[test]
    fn a_streamed_turn_with_thinking_text_and_a_tool_call_reads_back_whole() {
        let stream = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":120}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Checking \"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"SW1.\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_9\",\"name\":\"get_device\",\"input\":{}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"target\\\": \"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"SW1\\\"}\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":42}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        let mut p = Box::new(Parser::default());
        let mut deltas = Vec::new();
        for chunk in stream.as_bytes().chunks(17) {
            deltas.extend(p.feed(chunk).unwrap());
        }
        let r = p.finish().unwrap();
        assert_eq!(r.text, "Checking SW1.");
        assert_eq!(r.calls, [ToolCall { id: "toolu_9".into(), name: "get_device".into(), arguments: json!({ "target": "SW1" }) }]);
        assert_eq!(r.stop, Stop::ToolUse);
        assert_eq!(r.usage, Usage { input_tokens: 120, output_tokens: 42 });
        assert_eq!(r.native.as_ref().unwrap()[0]["signature"], "sig", "the thinking block is kept to send back");
        assert!(deltas.contains(&Delta::CallStarted("get_device".into())));
    }

    #[test]
    fn errors_are_said_in_words() {
        let p = Anthropic::new(zeroize::Zeroizing::new("k".into()), "claude-opus-5-5".into());
        assert!(matches!(p.error(401, None, r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#), ProviderError::BadKey(_)));
        assert!(matches!(p.error(400, None, r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low"}}"#), ProviderError::Billing(_)));
        assert!(matches!(p.error(404, None, r#"{"type":"error","error":{"type":"not_found_error","message":"model: x"}}"#), ProviderError::ModelNotFound(_)));
        assert_eq!(p.error(429, Some(12), r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#).to_string(), "Rate limited: slow down Try again in 12 seconds.");
        assert!(matches!(p.error(529, None, r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#), ProviderError::Overloaded(_)));
    }
}
