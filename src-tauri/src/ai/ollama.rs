//! Ollama, on this machine or the engineer's network: `POST
//! {base}/api/chat`, streamed as one JSON object a line, with tools.
//!
//! As documented (checked 2026-10-25): tools in the `function` shape, tool
//! calls arriving whole in `message.tool_calls` (arguments as an object,
//! no id — one is made here), results sent back as `role: tool` with the
//! `tool_name`, the last line `done: true` with `prompt_eval_count` and
//! `eval_count`. Models from `GET /api/tags`; whether one calls tools from
//! `POST /api/show`, whose `capabilities` lists `tools`. No key.

use serde_json::{json, Value};

use super::provider::*;
use crate::mcp::redact;

pub const DEFAULT_BASE: &str = "http://localhost:11434";

/// Models known to call tools well, to suggest when the chosen one cannot.
pub const TOOL_MODELS: &str = "qwen3, llama3.1, mistral-small3.1 or gpt-oss";

pub struct Ollama {
    pub model: String,
    pub base: String,
}

impl Ollama {
    fn base(&self) -> &str {
        self.base.trim_end_matches('/')
    }

    pub fn tags_url(&self) -> String {
        format!("{}/api/tags", self.base())
    }

    pub fn show_url(&self) -> String {
        format!("{}/api/show", self.base())
    }
}

impl Provider for Ollama {
    fn id(&self) -> &'static str {
        "ollama"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn request(&self, turn: &Turn) -> HttpRequest {
        let mut messages = vec![json!({ "role": "system", "content": turn.system })];
        for m in &turn.messages {
            match m {
                Message::User { text } => messages.push(json!({ "role": "user", "content": text })),
                Message::Assistant { text, calls, .. } => {
                    let mut msg = json!({ "role": "assistant", "content": text });
                    if !calls.is_empty() {
                        msg["tool_calls"] = Value::Array(calls.iter().map(|c| json!({ "function": { "name": c.name, "arguments": c.arguments } })).collect());
                    }
                    messages.push(msg);
                }
                Message::Tools { results } => {
                    for r in results {
                        messages.push(json!({ "role": "tool", "content": r.content, "tool_name": r.name }));
                    }
                }
            }
        }
        HttpRequest {
            url: format!("{}/api/chat", self.base()),
            headers: Vec::new(),
            body: json!({
                "model": self.model,
                "messages": messages,
                "tools": turn.tools.iter().map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.schema } })).collect::<Vec<_>>(),
                "stream": true,
                "options": { "num_predict": turn.max_tokens },
            }),
        }
    }

    fn parser(&self) -> Box<dyn StreamParser> {
        Box::new(Parser::default())
    }

    fn error(&self, status: u16, _retry_after: Option<u64>, body: &str) -> ProviderError {
        let message = error_message(body);
        let lower = message.to_ascii_lowercase();
        if lower.contains("does not support tools") {
            return ProviderError::NoTools(format!("The Ollama model {} cannot call tools, which the chat needs to read your project. Pull a model that can — {TOOL_MODELS} — and choose it in Settings → AI.", self.model));
        }
        match status {
            404 => ProviderError::ModelNotFound(format!("{} is not on this Ollama ({message}). Run `ollama pull {}` first.", self.model, self.model)),
            429 => ProviderError::RateLimited { retry_after: None, message },
            500..=599 => ProviderError::Overloaded(message),
            _ => ProviderError::Other(format!("Ollama answered {status}: {message}")),
        }
    }

    fn unreachable(&self, e: &str) -> ProviderError {
        ProviderError::NotRunning(format!("Ollama is not answering at {} ({e}). Start it (the Ollama app, or `ollama serve`) and try again.", self.base()))
    }
}

#[derive(Default)]
struct Parser {
    nd: NdJson,
    text: String,
    calls: Vec<ToolCall>,
    usage: Usage,
    done_reason: Option<String>,
    failed: Option<ProviderError>,
}

impl StreamParser for Parser {
    fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Delta>, ProviderError> {
        let mut out = Vec::new();
        for line in self.nd.feed(chunk) {
            if let Some(e) = line["error"].as_str() {
                self.failed = Some(ProviderError::Other(redact::shapes_only(e)));
                continue;
            }
            if let Some(t) = line["message"]["content"].as_str().filter(|t| !t.is_empty()) {
                self.text.push_str(t);
                out.push(Delta::Text(t.to_string()));
            }
            for tc in line["message"]["tool_calls"].as_array().into_iter().flatten() {
                let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                out.push(Delta::CallStarted(name.clone()));
                let arguments = match &tc["function"]["arguments"] {
                    Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({ "__invalid_json": true })),
                    v => v.clone(),
                };
                let id = format!("call_{}", self.calls.len());
                self.calls.push(ToolCall { id, name, arguments });
            }
            if line["done"] == true {
                self.usage = Usage { input_tokens: line["prompt_eval_count"].as_u64().unwrap_or(0), output_tokens: line["eval_count"].as_u64().unwrap_or(0) };
                self.done_reason = line["done_reason"].as_str().map(str::to_string);
            }
        }
        Ok(out)
    }

    fn finish(self: Box<Self>) -> Result<TurnResult, ProviderError> {
        if let Some(e) = self.failed {
            return Err(e);
        }
        let stop = match self.done_reason.as_deref() {
            Some("length") => Stop::MaxTokens,
            _ if !self.calls.is_empty() => Stop::ToolUse,
            _ => Stop::EndTurn,
        };
        Ok(TurnResult { text: self.text, calls: self.calls, native: None, usage: self.usage, stop })
    }
}

/// The model names `/api/tags` lists.
pub fn models_from(v: &Value) -> Vec<(String, String)> {
    v["models"].as_array().map(|a| a.iter().filter_map(|m| Some((m["name"].as_str()?.to_string(), m["details"]["parameter_size"].as_str().unwrap_or("").to_string()))).collect()).unwrap_or_default()
}

/// Whether `/api/show` says the model calls tools.
pub fn calls_tools(show: &Value) -> Option<bool> {
    show["capabilities"].as_array().map(|c| c.iter().any(|x| x == "tools"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_the_documented_shape() {
        let p = Ollama { model: "qwen3".into(), base: "http://localhost:11434/".into() };
        let turn = Turn {
            system: "sys".into(),
            messages: vec![
                Message::User { text: "hi".into() },
                Message::Assistant { text: String::new(), calls: vec![ToolCall { id: "call_0".into(), name: "get_device".into(), arguments: json!({ "target": "SW1" }) }], native: None, provider: "ollama".into() },
                Message::Tools { results: vec![ToolResult { id: "call_0".into(), name: "get_device".into(), content: "ok".into(), is_error: false }] },
            ],
            tools: vec![ToolDef { name: "get_device".into(), description: "d".into(), schema: json!({ "type": "object" }) }],
            max_tokens: 4000,
        };
        let r = p.request(&turn);
        assert_eq!(r.url, "http://localhost:11434/api/chat");
        assert!(r.headers.is_empty());
        assert_eq!(r.body["messages"][2]["tool_calls"][0]["function"]["arguments"]["target"], "SW1");
        assert_eq!(r.body["messages"][3], json!({ "role": "tool", "content": "ok", "tool_name": "get_device" }));
    }

    #[test]
    fn a_stream_with_a_call_and_the_closing_counts_reads_back_whole() {
        let stream = b"{\"message\":{\"role\":\"assistant\",\"content\":\"Let me check.\"},\"done\":false}\n{\"message\":{\"role\":\"assistant\",\"content\":\"\",\"tool_calls\":[{\"function\":{\"name\":\"get_device\",\"arguments\":{\"target\":\"SW1\"}}}]},\"done\":false}\n{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":169,\"eval_count\":15}\n";
        let mut p: Box<dyn StreamParser> = Box::new(Parser::default());
        for chunk in stream.chunks(7) {
            p.feed(chunk).unwrap();
        }
        let r = p.finish().unwrap();
        assert_eq!(r.text, "Let me check.");
        assert_eq!(r.calls[0].arguments["target"], "SW1");
        assert_eq!(r.stop, Stop::ToolUse);
        assert_eq!(r.usage, Usage { input_tokens: 169, output_tokens: 15 });
    }

    #[test]
    fn a_model_without_tools_and_one_not_pulled_are_said_plainly() {
        let p = Ollama { model: "llama2".into(), base: DEFAULT_BASE.into() };
        assert!(p.error(400, None, r#"{"error":"registry.ollama.ai/library/llama2:latest does not support tools"}"#).to_string().contains("cannot call tools"));
        assert!(p.error(404, None, r#"{"error":"model \"llama2\" not found, try pulling it first"}"#).to_string().contains("ollama pull llama2"));
        assert!(p.unreachable("connection refused").to_string().contains("Ollama is not answering"));
        assert_eq!(calls_tools(&json!({ "capabilities": ["completion", "tools"] })), Some(true));
        assert_eq!(calls_tools(&json!({ "capabilities": ["completion"] })), Some(false));
    }
}
