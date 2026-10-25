//! One shape for every model provider: a conversation in, a streamed turn
//! out, with tool calls.
//!
//! A provider is a [`Provider`]: it builds the HTTP request for a turn
//! ([`Provider::request`], pure, so a test can read the exact body it
//! would send) and reads its own stream ([`StreamParser`]). Sending is
//! common ([`send`]): from Rust only, redirects refused, the key in a
//! header and nowhere else, every text in the conversation through the
//! redaction gate first ([`outbound`]). The page never sees a key, a
//! request or a provider's raw reply.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mcp::redact;

/// A tool the model may call: the MCP tool list, as each provider spells
/// it.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
}

/// One message of a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "role")]
pub enum Message {
    User { text: String },
    /// What the model said. `native` is the provider's own form of it
    /// (Anthropic's content blocks with their thinking and signatures,
    /// OpenAI's output items with their reasoning), sent back as it came
    /// when the conversation goes on with the same provider.
    Assistant { text: String, calls: Vec<ToolCall>, native: Option<Value>, provider: String },
    Tools { results: Vec<ToolResult> },
}

/// What one turn asks for.
#[derive(Debug, Clone)]
pub struct Turn {
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    EndTurn,
    ToolUse,
    MaxTokens,
    Refused(String),
    Other(String),
}

/// A turn, read to its end.
#[derive(Debug, Clone)]
pub struct TurnResult {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub native: Option<Value>,
    pub usage: Usage,
    pub stop: Stop,
}

/// Why a provider could not answer, in words a person can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    BadKey(String),
    Billing(String),
    ModelNotFound(String),
    NotRunning(String),
    RateLimited { retry_after: Option<u64>, message: String },
    Overloaded(String),
    NoTools(String),
    Network(String),
    Other(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::BadKey(m) => write!(f, "The API key was refused: {m} Paste a working key in Settings → AI → AI providers."),
            ProviderError::Billing(m) => write!(f, "The provider account has no credit or quota left: {m}"),
            ProviderError::ModelNotFound(m) => write!(f, "That model is not available: {m} Choose another in Settings → AI."),
            ProviderError::NotRunning(m) => write!(f, "{m}"),
            ProviderError::RateLimited { retry_after: Some(s), message } => write!(f, "Rate limited: {message} Try again in {s} seconds."),
            ProviderError::RateLimited { retry_after: None, message } => write!(f, "Rate limited: {message} Try again in a minute."),
            ProviderError::Overloaded(m) => write!(f, "The provider is overloaded or failing right now: {m} Try again shortly."),
            ProviderError::NoTools(m) => write!(f, "{m}"),
            ProviderError::Network(m) => write!(f, "Could not reach the provider: {m}"),
            ProviderError::Other(m) => write!(f, "{m}"),
        }
    }
}

/// Something a turn's stream produced, for the panel as it arrives.
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    Text(String),
    /// A tool call has started (its arguments may still be arriving).
    CallStarted(String),
}

/// A provider's own stream, read chunk by chunk.
pub trait StreamParser: Send {
    fn feed(&mut self, chunk: &[u8]) -> Result<Vec<Delta>, ProviderError>;
    fn finish(self: Box<Self>) -> Result<TurnResult, ProviderError>;
}

/// The request, as it would go on the wire.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub url: String,
    /// Headers, the key's among them; never logged, never shown.
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

pub trait Provider: Send + Sync {
    /// `anthropic`, `openai`, `ollama`.
    fn id(&self) -> &'static str;
    fn model(&self) -> &str;
    fn request(&self, turn: &Turn) -> HttpRequest;
    fn parser(&self) -> Box<dyn StreamParser>;
    /// What an HTTP error with this status and body means.
    fn error(&self, status: u16, retry_after: Option<u64>, body: &str) -> ProviderError;
    /// What failing to connect means (Ollama: it is not running).
    fn unreachable(&self, e: &str) -> ProviderError {
        ProviderError::Network(e.to_string())
    }
}

/// `turn` with every text in it through the redaction gate: what the
/// engineer typed, and what tools returned (already gated once on their
/// way out of the tool; the second pass costs nothing and catches a
/// secret typed into the question).
pub fn outbound(turn: &Turn, secrets: &redact::Secrets) -> Turn {
    let gate = |t: &str| redact::gate(t, secrets).unwrap_or_else(|_| redact::WITHHELD.to_string());
    let messages = turn
        .messages
        .iter()
        .map(|m| match m {
            Message::User { text } => Message::User { text: gate(text) },
            Message::Assistant { text, calls, native, provider } => Message::Assistant { text: gate(text), calls: calls.clone(), native: native.clone(), provider: provider.clone() },
            Message::Tools { results } => Message::Tools { results: results.iter().map(|r| ToolResult { content: gate(&r.content), ..r.clone() }).collect() },
        })
        .collect();
    Turn { system: turn.system.clone(), messages, tools: turn.tools.clone(), max_tokens: turn.max_tokens }
}

/// The request a provider would send for `turn`, after the gate. The only
/// way a request is built.
pub fn outbound_request(provider: &dyn Provider, turn: &Turn, secrets: &redact::Secrets) -> HttpRequest {
    provider.request(&outbound(turn, secrets))
}

/// The HTTP client every provider call goes through: no redirects, a
/// connect limit, rustls.
pub fn client() -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("corelens-desktop-ai/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| ProviderError::Other(format!("The HTTP client could not start: {e}")))
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers.get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<f64>().ok()).map(|s| s.ceil().max(1.0) as u64)
}

/// How long one chunk of a stream may take to arrive.
const IDLE: Duration = Duration::from_secs(180);

/// Sends a turn and reads its stream, handing each delta to `on_delta`.
/// A rate limit or an overload is retried twice, waiting as told (at most
/// 30 s) before it is reported.
pub async fn send(provider: &dyn Provider, turn: &Turn, secrets: &redact::Secrets, on_delta: &(dyn Fn(Delta) + Send + Sync), last_body: &(dyn Fn(&Value) + Send + Sync)) -> Result<TurnResult, ProviderError> {
    let req = outbound_request(provider, turn, secrets);
    last_body(&req.body);
    let http = client()?;
    let body = serde_json::to_vec(&req.body).map_err(|e| ProviderError::Other(e.to_string()))?;
    let started = std::time::Instant::now();
    let mut attempt = 0;
    let response = loop {
        let mut rb = http.post(&req.url).header("content-type", "application/json");
        for (k, v) in &req.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        let r = rb.body(body.clone()).send().await.map_err(|e| provider.unreachable(&clean_error(&e)))?;
        let status = r.status().as_u16();
        if (200..300).contains(&status) {
            break r;
        }
        let wait = retry_after(r.headers());
        let text = r.text().await.unwrap_or_default();
        let err = provider.error(status, wait, &text);
        let retryable = matches!(err, ProviderError::RateLimited { .. } | ProviderError::Overloaded(_));
        if retryable && attempt < 2 {
            attempt += 1;
            tokio::time::sleep(Duration::from_secs(wait.unwrap_or(2 * attempt).min(30))).await;
            continue;
        }
        crate::applog::warn("ai", &format!("{} answered HTTP {status}", provider.id()));
        return Err(err);
    };
    let mut parser = provider.parser();
    let mut response = response;
    let mut bytes = 0usize;
    loop {
        match tokio::time::timeout(IDLE, response.chunk()).await {
            Err(_) => return Err(ProviderError::Network(format!("{} stopped sending for {} seconds.", provider.id(), IDLE.as_secs()))),
            Ok(Err(e)) => return Err(ProviderError::Network(clean_error(&e))),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(chunk))) => {
                bytes += chunk.len();
                for d in parser.feed(&chunk)? {
                    on_delta(d);
                }
            }
        }
    }
    let result = parser.finish()?;
    crate::applog::verbose_with("ai", || format!("{} {}: {} bytes streamed in {} ms, {} in / {} out tokens, {} tool call(s)", provider.id(), provider.model(), bytes, started.elapsed().as_millis(), result.usage.input_tokens, result.usage.output_tokens, result.calls.len()));
    Ok(result)
}

/// A reqwest error as a sentence. No key is ever in a URL here (they go
/// in headers), and the shapes of keys are removed all the same.
pub fn clean_error(e: &reqwest::Error) -> String {
    redact::shapes_only(&e.to_string())
}

/// A GET to a provider, for its model list or a health check.
pub async fn get(provider: &dyn Provider, url: &str, headers: &[(String, String)]) -> Result<Value, ProviderError> {
    let http = client()?;
    let mut rb = http.get(url);
    for (k, v) in headers {
        rb = rb.header(k.as_str(), v.as_str());
    }
    let r = tokio::time::timeout(Duration::from_secs(20), rb.send())
        .await
        .map_err(|_| ProviderError::Network("no answer within 20 seconds".into()))?
        .map_err(|e| provider.unreachable(&clean_error(&e)))?;
    let status = r.status().as_u16();
    let wait = retry_after(r.headers());
    let text = r.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(provider.error(status, wait, &text));
    }
    serde_json::from_str(&text).map_err(|e| ProviderError::Other(format!("The provider's answer was not JSON: {e}")))
}

/// A POST that is not a turn (Ollama's `/api/show`).
pub async fn post_json(provider: &dyn Provider, url: &str, body: &Value) -> Result<Value, ProviderError> {
    let http = client()?;
    let r = tokio::time::timeout(Duration::from_secs(20), http.post(url).header("content-type", "application/json").body(body.to_string()).send())
        .await
        .map_err(|_| ProviderError::Network("no answer within 20 seconds".into()))?
        .map_err(|e| provider.unreachable(&clean_error(&e)))?;
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(provider.error(status, None, &text));
    }
    serde_json::from_str(&text).map_err(|e| ProviderError::Other(format!("The provider's answer was not JSON: {e}")))
}

/// Server-sent events: `event:`/`data:` lines, a blank line between
/// events, split anywhere across chunks.
#[derive(Default)]
pub struct Sse {
    buffer: String,
    event: String,
    data: String,
}

impl Sse {
    /// The complete events in `chunk` and what came before it: `(event,
    /// data)`.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<(String, String)> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut out = Vec::new();
        while let Some(nl) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=nl).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() || !self.event.is_empty() {
                    out.push((std::mem::take(&mut self.event), std::mem::take(&mut self.data)));
                }
            } else if let Some(v) = line.strip_prefix("event:") {
                self.event = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(v.strip_prefix(' ').unwrap_or(v));
            }
        }
        out
    }
}

/// Newline-delimited JSON (Ollama), split anywhere across chunks.
#[derive(Default)]
pub struct NdJson {
    buffer: String,
}

impl NdJson {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Value> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut out = Vec::new();
        while let Some(nl) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=nl).collect();
            if let Ok(v) = serde_json::from_str(line.trim()) {
                out.push(v);
            }
        }
        out
    }
}

/// A provider's error body's message, whatever its shape.
pub fn error_message(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let m = v["error"]["message"].as_str().or_else(|| v["error"].as_str()).or_else(|| v["message"].as_str()).unwrap_or("");
    let m = if m.is_empty() { body.trim() } else { m };
    let m: String = m.chars().take(300).collect();
    redact::shapes_only(&m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_split_anywhere_come_back_whole() {
        let stream = b"event: a\ndata: {\"x\":1}\n\nevent: b\ndata: line one\ndata: line two\n\n";
        for cut in 1..stream.len() {
            let mut sse = Sse::default();
            let mut got = sse.feed(&stream[..cut]);
            got.extend(sse.feed(&stream[cut..]));
            assert_eq!(got, [("a".to_string(), "{\"x\":1}".to_string()), ("b".to_string(), "line one\nline two".to_string())], "cut at {cut}");
        }
    }

    #[test]
    fn json_lines_split_anywhere_come_back_whole() {
        let stream = b"{\"a\":1}\n{\"b\":\"x\\ny\"}\n";
        for cut in 1..stream.len() {
            let mut nd = NdJson::default();
            let mut got = nd.feed(&stream[..cut]);
            got.extend(nd.feed(&stream[cut..]));
            assert_eq!(got.len(), 2, "cut at {cut}");
        }
    }

    #[test]
    fn an_error_message_never_carries_a_key() {
        let m = error_message(r#"{"error":{"type":"authentication_error","message":"invalid x-api-key sk-ant-api03-FIXTUREfixtureFIXTURE"}}"#);
        assert!(!m.contains("sk-ant-api03"), "{m}");
    }
}
