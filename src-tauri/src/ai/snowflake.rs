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

    pub fn headers(&self) -> Vec<(String, String)> {
        let mut h = self.inner.headers();
        h.push(("x-snowflake-authorization-token-type".into(), "PROGRAMMATIC_ACCESS_TOKEN".into()));
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
        r.headers = self.headers();
        r.body["max_completion_tokens"] = json!(turn.max_tokens);
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

    #[test]
    fn the_model_list_is_read_in_any_likely_shape() {
        assert_eq!(models_from(&json!(["mistral-large2", "claude-sonnet-5-5"])), ["claude-sonnet-5-5", "mistral-large2"]);
        assert_eq!(models_from(&json!({ "models": [{ "name": "claude-opus-5-5" }, { "name": "llama3.3-70b" }] })), ["claude-opus-5-5", "llama3.3-70b"]);
        assert_eq!(models_from(&json!({ "data": [{ "id": "openai-gpt-5" }] })), ["openai-gpt-5"]);
        assert!(models_from(&json!({ "something": "else" })).is_empty());
        assert!(calls_tools("claude-sonnet-5-5") && calls_tools("openai-gpt-5") && !calls_tools("llama3.3-70b"));
    }
}
