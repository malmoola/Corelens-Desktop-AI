//! OpenAI, streamed, with tools — in two dialects.
//!
//! **Responses** (`POST {base}/responses`, the default): what OpenAI's
//! current models use for tool calling, as documented (checked
//! 2026-10-25, the guide and the official SDK's types): function tools at
//! the top level of `tools`, `function_call` output items answered with
//! `function_call_output` input items, every output item — reasoning with
//! its `encrypted_content` included — passed back as it came, `store:
//! false` so the conversation is held here and nowhere else. Stream:
//! `response.output_text.delta`, `response.output_item.added` / `.done`,
//! `response.completed` / `.incomplete` / `.failed`, `error`.
//!
//! **Chat Completions** (`POST {base}/chat/completions`): for the
//! OpenAI-compatible servers a custom base URL points at. `tools` of type
//! `function`, `tool_calls` streamed as fragments by index, `role: tool`
//! results, `stream_options.include_usage` for the token count.
//!
//! The key goes in `Authorization: Bearer`, or in `api-key` for an Azure
//! host.

use serde_json::{json, Value};

use super::provider::*;
use crate::mcp::redact;

pub const API: &str = "https://api.openai.com/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Responses,
    Chat,
}

pub struct OpenAi {
    pub key: zeroize::Zeroizing<String>,
    pub model: String,
    pub base: String,
    pub style: Style,
}

impl OpenAi {
    pub fn headers(&self) -> Vec<(String, String)> {
        let host = self.base.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("").to_ascii_lowercase();
        if host.ends_with(".azure.com") || host.ends_with(".azure.us") {
            vec![("api-key".into(), self.key.to_string())]
        } else {
            vec![("authorization".into(), format!("Bearer {}", self.key.as_str()))]
        }
    }

    fn responses_input(turn: &Turn) -> Vec<Value> {
        let mut out = Vec::new();
        for m in &turn.messages {
            match m {
                Message::User { text } => out.push(json!({ "role": "user", "content": [{ "type": "input_text", "text": text }] })),
                Message::Assistant { text, calls, native, provider } => match native {
                    Some(Value::Array(items)) if provider == "openai" => out.extend(items.iter().cloned()),
                    _ => {
                        if !text.is_empty() {
                            out.push(json!({ "role": "assistant", "content": [{ "type": "output_text", "text": text }] }));
                        }
                        for c in calls {
                            out.push(json!({ "type": "function_call", "call_id": c.id, "name": c.name, "arguments": c.arguments.to_string() }));
                        }
                    }
                },
                Message::Tools { results } => {
                    for r in results {
                        out.push(json!({ "type": "function_call_output", "call_id": r.id, "output": r.content }));
                    }
                }
            }
        }
        out
    }

    fn chat_messages(turn: &Turn) -> Vec<Value> {
        let mut out = vec![json!({ "role": "system", "content": turn.system })];
        for m in &turn.messages {
            match m {
                Message::User { text } => out.push(json!({ "role": "user", "content": text })),
                Message::Assistant { text, calls, .. } => {
                    let mut msg = json!({ "role": "assistant", "content": text });
                    if !calls.is_empty() {
                        msg["tool_calls"] = Value::Array(calls.iter().map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.arguments.to_string() } })).collect());
                    }
                    out.push(msg);
                }
                Message::Tools { results } => {
                    for r in results {
                        out.push(json!({ "role": "tool", "tool_call_id": r.id, "content": r.content }));
                    }
                }
            }
        }
        out
    }
}

impl Provider for OpenAi {
    fn id(&self) -> &'static str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn request(&self, turn: &Turn) -> HttpRequest {
        let base = self.base.trim_end_matches('/');
        match self.style {
            Style::Responses => HttpRequest {
                url: format!("{base}/responses"),
                headers: self.headers(),
                body: json!({
                    "model": self.model,
                    "instructions": turn.system,
                    "input": OpenAi::responses_input(turn),
                    "tools": turn.tools.iter().map(|t| json!({ "type": "function", "name": t.name, "description": t.description, "parameters": t.schema })).collect::<Vec<_>>(),
                    "tool_choice": "auto",
                    "stream": true,
                    "store": false,
                    "include": ["reasoning.encrypted_content"],
                    "max_output_tokens": turn.max_tokens,
                }),
            },
            Style::Chat => HttpRequest {
                url: format!("{base}/chat/completions"),
                headers: self.headers(),
                body: json!({
                    "model": self.model,
                    "messages": OpenAi::chat_messages(turn),
                    "tools": turn.tools.iter().map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.schema } })).collect::<Vec<_>>(),
                    "tool_choice": "auto",
                    "stream": true,
                    "stream_options": { "include_usage": true },
                }),
            },
        }
    }

    fn parser(&self) -> Box<dyn StreamParser> {
        match self.style {
            Style::Responses => Box::new(ResponsesParser::default()),
            Style::Chat => Box::new(ChatParser::default()),
        }
    }

    fn error(&self, status: u16, retry_after: Option<u64>, body: &str) -> ProviderError {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let code = v["error"]["code"].as_str().or_else(|| v["error"]["type"].as_str()).unwrap_or("").to_string();
        let message = error_message(body);
        match (status, code.as_str()) {
            (401, _) | (_, "invalid_api_key") => ProviderError::BadKey(message),
            (_, "insufficient_quota") | (402, _) => ProviderError::Billing(message),
            (404, _) | (_, "model_not_found") => ProviderError::ModelNotFound(format!("{} ({message})", self.model)),
            (429, _) => ProviderError::RateLimited { retry_after, message },
            (500..=599, _) => ProviderError::Overloaded(message),
            _ => ProviderError::Other(format!("OpenAI answered {status}: {message}")),
        }
    }
}

#[derive(Default)]
struct ResponsesParser {
    sse: Sse,
    items: Vec<Value>,
    text: String,
    usage: Usage,
    incomplete: Option<String>,
    failed: Option<ProviderError>,
}

impl StreamParser for ResponsesParser {
    fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Delta>, ProviderError> {
        let mut out = Vec::new();
        for (_, data) in self.sse.feed(chunk) {
            let Ok(ev) = serde_json::from_str::<Value>(&data) else { continue };
            match ev["type"].as_str().unwrap_or("") {
                "response.output_text.delta" => {
                    let d = ev["delta"].as_str().unwrap_or("");
                    self.text.push_str(d);
                    out.push(Delta::Text(d.to_string()));
                }
                "response.output_item.added" if ev["item"]["type"] == "function_call" => out.push(Delta::CallStarted(ev["item"]["name"].as_str().unwrap_or("").to_string())),
                // The finished item, which is what goes back next turn (a
                // reasoning item's encrypted content is complete only here).
                "response.output_item.done" => self.items.push(ev["item"].clone()),
                "response.completed" => {
                    let u = &ev["response"]["usage"];
                    self.usage = Usage { input_tokens: u["input_tokens"].as_u64().unwrap_or(0), output_tokens: u["output_tokens"].as_u64().unwrap_or(0) };
                }
                "response.incomplete" => {
                    let u = &ev["response"]["usage"];
                    self.usage = Usage { input_tokens: u["input_tokens"].as_u64().unwrap_or(0), output_tokens: u["output_tokens"].as_u64().unwrap_or(0) };
                    self.incomplete = Some(ev["response"]["incomplete_details"]["reason"].as_str().unwrap_or("incomplete").to_string());
                }
                "response.failed" => {
                    let m = redact::shapes_only(ev["response"]["error"]["message"].as_str().unwrap_or("the response failed"));
                    self.failed = Some(ProviderError::Other(m));
                }
                "error" => {
                    let m = redact::shapes_only(ev["message"].as_str().unwrap_or("the stream ended with an error"));
                    self.failed = Some(match ev["code"].as_str() {
                        Some("rate_limit_exceeded") => ProviderError::RateLimited { retry_after: None, message: m },
                        Some("insufficient_quota") => ProviderError::Billing(m),
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
        let calls: Vec<ToolCall> = self
            .items
            .iter()
            .filter(|i| i["type"] == "function_call")
            .map(|i| ToolCall {
                id: i["call_id"].as_str().unwrap_or("").into(),
                name: i["name"].as_str().unwrap_or("").into(),
                arguments: serde_json::from_str(i["arguments"].as_str().unwrap_or("{}")).unwrap_or_else(|_| json!({ "__invalid_json": true })),
            })
            .collect();
        let stop = match self.incomplete.as_deref() {
            Some("max_output_tokens") => Stop::MaxTokens,
            Some("content_filter") => Stop::Refused("the provider's content filter".into()),
            Some(other) => Stop::Other(other.into()),
            None if calls.is_empty() => Stop::EndTurn,
            None => Stop::ToolUse,
        };
        Ok(TurnResult { text: self.text, calls, native: Some(Value::Array(self.items)), usage: self.usage, stop })
    }
}

/// One tool call as its fragments arrive.
#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct ChatParser {
    sse: Sse,
    text: String,
    /// Tool calls in the order they began.
    calls: Vec<PendingCall>,
    /// Which call an id, or a stream's call number, belongs to. An id wins:
    /// Cortex numbers every one of Claude's calls 0 and tells them apart
    /// only by id.
    by_id: std::collections::HashMap<String, usize>,
    by_index: std::collections::HashMap<u64, usize>,
    finish: Option<String>,
    usage: Usage,
}

impl ChatParser {
    /// The call a fragment belongs to, begun if it is new.
    fn call_for(&mut self, id: Option<&str>, index: Option<u64>) -> usize {
        let id = id.map(str::trim).filter(|i| !i.is_empty());
        let found = match (id, index) {
            (Some(id), _) => self.by_id.get(id).copied(),
            (None, Some(i)) => self.by_index.get(&i).copied(),
            (None, None) => self.calls.len().checked_sub(1),
        };
        let slot = found.unwrap_or_else(|| {
            self.calls.push(PendingCall { id: id.unwrap_or("").to_string(), ..Default::default() });
            self.calls.len() - 1
        });
        if let Some(id) = id {
            self.by_id.insert(id.to_string(), slot);
        }
        if let Some(i) = index {
            self.by_index.insert(i, slot);
        }
        slot
    }

    /// A fragment of one call: the name is taken once, when the call first
    /// names itself, and never added to; only the arguments are joined.
    fn fragment(&mut self, slot: usize, name: Option<&str>, arguments: Option<&str>, out: &mut Vec<Delta>) {
        let call = &mut self.calls[slot];
        if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
            if call.name.is_empty() {
                call.name = n.to_string();
                out.push(Delta::CallStarted(n.to_string()));
            }
        }
        if let Some(a) = arguments.filter(|a| !a.is_empty()) {
            // A stream that resends the whole input rather than the next
            // piece of it: the input is replaced, not doubled.
            let whole = |t: &str| serde_json::from_str::<Value>(t).is_ok_and(|v| v.is_object());
            if whole(&call.arguments) && whole(a) {
                call.arguments = a.to_string();
            } else {
                call.arguments.push_str(a);
            }
        }
    }
}

impl StreamParser for ChatParser {
    fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Delta>, ProviderError> {
        let mut out = Vec::new();
        for (_, data) in self.sse.feed(chunk) {
            if data.trim() == "[DONE]" {
                continue;
            }
            let Ok(ev) = serde_json::from_str::<Value>(&data) else { continue };
            if let Some(err) = ev.get("error").filter(|e| !e.is_null()) {
                return Err(ProviderError::Other(redact::shapes_only(err["message"].as_str().or_else(|| err.as_str()).unwrap_or("the stream ended with an error"))));
            }
            if let Some(u) = ev.get("usage").filter(|u| u.is_object()) {
                self.usage = Usage { input_tokens: u["prompt_tokens"].as_u64().or_else(|| u["input_tokens"].as_u64()).unwrap_or(self.usage.input_tokens), output_tokens: u["completion_tokens"].as_u64().or_else(|| u["output_tokens"].as_u64()).unwrap_or(self.usage.output_tokens) };
            }
            // Anthropic's events, which Cortex can pass through for Claude.
            match ev["type"].as_str().unwrap_or("") {
                "content_block_start" if ev["content_block"]["type"] == "tool_use" => {
                    let b = &ev["content_block"];
                    let slot = self.call_for(b["id"].as_str(), ev["index"].as_u64());
                    let input = b["input"].as_object().filter(|o| !o.is_empty()).map(|o| Value::Object(o.clone()).to_string());
                    self.fragment(slot, b["name"].as_str(), input.as_deref(), &mut out);
                    continue;
                }
                "content_block_delta" => {
                    let d = &ev["delta"];
                    match d["type"].as_str().unwrap_or("") {
                        "input_json_delta" => {
                            let slot = self.call_for(None, ev["index"].as_u64());
                            self.fragment(slot, None, d["partial_json"].as_str(), &mut out);
                        }
                        "text_delta" => {
                            if let Some(t) = d["text"].as_str() {
                                self.text.push_str(t);
                                out.push(Delta::Text(t.to_string()));
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                "message_delta" => {
                    match ev["delta"]["stop_reason"].as_str() {
                        Some("tool_use") => self.finish = Some("tool_calls".into()),
                        Some("max_tokens") => self.finish = Some("length".into()),
                        Some(other) => self.finish = Some(other.into()),
                        None => {}
                    }
                    continue;
                }
                _ => {}
            }
            for choice in ev["choices"].as_array().into_iter().flatten() {
                let d = &choice["delta"];
                if d["type"] == "tool_use" {
                    // Cortex's own `inference:complete` delta: the id and
                    // name on every fragment, the input in pieces.
                    let slot = self.call_for(d["tool_use_id"].as_str(), None);
                    self.fragment(slot, d["name"].as_str(), d["input"].as_str(), &mut out);
                } else if let Some(t) = d["content"].as_str().or_else(|| if d["type"] == "text" { d["text"].as_str() } else { None }) {
                    self.text.push_str(t);
                    out.push(Delta::Text(t.to_string()));
                }
                for tc in d["tool_calls"].as_array().into_iter().flatten() {
                    let slot = self.call_for(tc["id"].as_str(), tc["index"].as_u64());
                    self.fragment(slot, tc["function"]["name"].as_str(), tc["function"]["arguments"].as_str(), &mut out);
                }
                if let Some(f) = choice["finish_reason"].as_str() {
                    self.finish = Some(f.to_string());
                }
            }
        }
        Ok(out)
    }

    fn finish(self: Box<Self>) -> Result<TurnResult, ProviderError> {
        let calls: Vec<ToolCall> = self
            .calls
            .into_iter()
            .enumerate()
            // The arguments are read once, whole.
            .map(|(i, c)| ToolCall {
                id: if c.id.is_empty() { format!("call_{i}") } else { c.id },
                name: c.name,
                arguments: if c.arguments.trim().is_empty() { json!({}) } else { serde_json::from_str(&c.arguments).unwrap_or_else(|_| json!({ "__invalid_json": true })) },
            })
            .collect();
        let stop = match self.finish.as_deref() {
            Some("length") => Stop::MaxTokens,
            Some("content_filter") => Stop::Refused("the provider's content filter".into()),
            Some("tool_calls") => Stop::ToolUse,
            _ if !calls.is_empty() => Stop::ToolUse,
            _ => Stop::EndTurn,
        };
        Ok(TurnResult { text: self.text, calls, native: None, usage: self.usage, stop })
    }
}

/// The model ids `GET {base}/models` lists, sorted.
pub fn models_from(v: &Value) -> Vec<(String, String)> {
    let mut ids: Vec<(String, String)> = v["data"].as_array().map(|a| a.iter().filter_map(|m| Some((m["id"].as_str()?.to_string(), String::new()))).collect()).unwrap_or_default();
    ids.sort();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn() -> Turn {
        Turn {
            system: "sys".into(),
            messages: vec![
                Message::User { text: "hi".into() },
                Message::Assistant { text: String::new(), calls: vec![ToolCall { id: "call_1".into(), name: "get_device".into(), arguments: json!({ "target": "SW1" }) }], native: None, provider: "anthropic".into() },
                Message::Tools { results: vec![ToolResult { id: "call_1".into(), name: "get_device".into(), content: "ok".into(), is_error: false }] },
            ],
            tools: vec![ToolDef { name: "get_device".into(), description: "d".into(), schema: json!({ "type": "object" }) }],
            max_tokens: 4000,
        }
    }

    fn provider(style: Style, base: &str) -> OpenAi {
        OpenAi { key: zeroize::Zeroizing::new("sk-proj-fixture-key-000000000000".into()), model: "gpt-6-astra".into(), base: base.into(), style }
    }

    #[test]
    fn a_responses_request_is_the_documented_shape() {
        let r = provider(Style::Responses, API).request(&turn());
        assert_eq!(r.url, "https://api.openai.com/v1/responses");
        assert_eq!(r.body["store"], false);
        assert_eq!(r.body["tools"][0]["type"], "function");
        assert_eq!(r.body["tools"][0]["name"], "get_device", "flat, not nested under function");
        assert_eq!(r.body["input"][1]["type"], "function_call");
        assert_eq!(r.body["input"][2], json!({ "type": "function_call_output", "call_id": "call_1", "output": "ok" }));
        assert_eq!(r.headers, [("authorization".to_string(), "Bearer sk-proj-fixture-key-000000000000".to_string())]);
        assert!(!r.body.to_string().contains("sk-proj"));
    }

    #[test]
    fn a_chat_completions_request_and_an_azure_host_get_their_own_shapes() {
        let r = provider(Style::Chat, "https://lab.openai.azure.com/openai/v1/").request(&turn());
        assert_eq!(r.url, "https://lab.openai.azure.com/openai/v1/chat/completions");
        assert_eq!(r.headers[0].0, "api-key");
        assert_eq!(r.body["messages"][0]["role"], "system");
        assert_eq!(r.body["messages"][2]["tool_calls"][0]["function"]["arguments"], "{\"target\":\"SW1\"}");
        assert_eq!(r.body["messages"][3]["role"], "tool");
        assert_eq!(r.body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn a_responses_stream_with_reasoning_text_and_a_call_reads_back_whole() {
        let ev = |v: Value| format!("event: {}\ndata: {}\n\n", v["type"].as_str().unwrap(), v);
        let stream = [
            ev(json!({ "type": "response.created", "response": {} })),
            ev(json!({ "type": "response.output_item.done", "output_index": 0, "item": { "type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "enc" } })),
            ev(json!({ "type": "response.output_text.delta", "item_id": "m1", "output_index": 1, "content_index": 0, "delta": "Looking." })),
            ev(json!({ "type": "response.output_item.added", "output_index": 2, "item": { "type": "function_call", "call_id": "call_7", "name": "get_device", "arguments": "" } })),
            ev(json!({ "type": "response.function_call_arguments.delta", "item_id": "fc", "output_index": 2, "delta": "{\"target\":" })),
            ev(json!({ "type": "response.output_item.done", "output_index": 2, "item": { "type": "function_call", "call_id": "call_7", "name": "get_device", "arguments": "{\"target\":\"SW1\"}" } })),
            ev(json!({ "type": "response.completed", "response": { "usage": { "input_tokens": 50, "output_tokens": 9 } } })),
        ]
        .concat();
        let mut p: Box<dyn StreamParser> = Box::new(ResponsesParser::default());
        for chunk in stream.as_bytes().chunks(23) {
            p.feed(chunk).unwrap();
        }
        let r = p.finish().unwrap();
        assert_eq!(r.text, "Looking.");
        assert_eq!(r.calls[0].arguments, json!({ "target": "SW1" }));
        assert_eq!(r.stop, Stop::ToolUse);
        assert_eq!(r.native.unwrap()[0]["encrypted_content"], "enc");
        assert_eq!(r.usage, Usage { input_tokens: 50, output_tokens: 9 });
    }

    #[test]
    fn a_chat_stream_with_fragmented_tool_calls_reads_back_whole() {
        let stream = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hm\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",\"function\":{\"name\":\"get_device\",\"arguments\":\"{\\\"tar\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"get\\\":\\\"SW1\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":30,\"completion_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );
        let mut p: Box<dyn StreamParser> = Box::new(ChatParser::default());
        for chunk in stream.as_bytes().chunks(11) {
            p.feed(chunk).unwrap();
        }
        let r = p.finish().unwrap();
        assert_eq!(r.calls, [ToolCall { id: "call_a".into(), name: "get_device".into(), arguments: json!({ "target": "SW1" }) }]);
        assert_eq!((r.text.as_str(), r.stop, r.usage), ("Hm", Stop::ToolUse, Usage { input_tokens: 30, output_tokens: 5 }));
    }

    #[test]
    fn errors_are_said_in_words() {
        let p = provider(Style::Responses, API);
        assert!(matches!(p.error(401, None, r#"{"error":{"message":"Incorrect API key provided","code":"invalid_api_key"}}"#), ProviderError::BadKey(_)));
        assert!(matches!(p.error(429, None, r#"{"error":{"message":"You exceeded your current quota","code":"insufficient_quota"}}"#), ProviderError::Billing(_)));
        assert!(matches!(p.error(429, Some(3), r#"{"error":{"message":"Rate limit reached","code":"rate_limit_exceeded"}}"#), ProviderError::RateLimited { retry_after: Some(3), .. }));
        assert!(matches!(p.error(404, None, r#"{"error":{"message":"The model does not exist","code":"model_not_found"}}"#), ProviderError::ModelNotFound(_)));
    }
}
