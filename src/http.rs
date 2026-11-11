//! One HTTP client and one error type for every vendor call.
//!
//! The error keeps the reason apart from the text, because the dashboard acts
//! on it: an auth failure stops polling that account until its login changes,
//! a rate limit backs off, a network failure just waits for the next tick.

use serde_json::Value;
use std::fmt;
use std::time::Duration;

pub const USER_AGENT: &str = concat!("cswap/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// No login on disk for this account.
    NoLogin,
    /// The stored token is past its expiry and cswap may not refresh it.
    Expired,
    /// The vendor rejected the token (401/403, or a dead refresh token).
    Rejected,
    RateLimited,
    /// The vendor answered 5xx.
    Server,
    /// Could not reach the vendor at all.
    Network,
    /// The answer did not have the shape this version of cswap knows.
    Shape,
    /// This harness has no usage API cswap can call.
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct FetchError {
    pub why: Why,
    pub text: String,
}

impl FetchError {
    pub fn new(why: Why, text: impl Into<String>) -> FetchError {
        FetchError {
            why,
            text: text.into(),
        }
    }

    /// Will asking again soon get the same answer? True for everything that
    /// needs the user to do something first.
    pub fn needs_user(&self) -> bool {
        matches!(
            self.why,
            Why::NoLogin | Why::Expired | Why::Rejected | Why::Unsupported
        )
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for FetchError {}

pub type Fetch<T> = Result<T, FetchError>;

/// Bounded on purpose: the dashboard fans out one call per account, so a
/// hanging endpoint must fail fast. No retries: the next tick is the retry.
pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(6))
        .timeout(Duration::from_secs(20))
        .try_proxy_from_env(true)
        .user_agent(USER_AGENT)
        .build()
}

/// The vendor's own error text, when the body carries one.
fn api_message(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed: Option<Value> = serde_json::from_str(trimmed).ok();
    let text = parsed
        .as_ref()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .or_else(|| v.get("error_description").and_then(Value::as_str))
                .or_else(|| v.get("detail").and_then(Value::as_str))
                .or_else(|| v.get("message").and_then(Value::as_str))
                .or_else(|| v.get("error").and_then(Value::as_str))
                .map(String::from)
        })
        .unwrap_or_else(|| trimmed.chars().take(100).collect());
    Some(text)
}

/// Turn a ureq failure into a `FetchError` that says what to do about it.
pub fn explain(err: ureq::Error) -> FetchError {
    match err {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let (why, hint) = match code {
                401 | 403 => (Why::Rejected, "token rejected"),
                429 => (Why::RateLimited, "rate limited"),
                500..=599 => (Why::Server, "server error"),
                _ => (Why::Shape, "unexpected answer"),
            };
            let detail = api_message(&body)
                .map(|m| format!(": {m}"))
                .unwrap_or_default();
            FetchError::new(why, format!("{hint} (HTTP {code}{detail})"))
        }
        ureq::Error::Transport(t) => {
            let text = t.to_string();
            let short = text.split(": ").last().unwrap_or(&text).to_string();
            FetchError::new(Why::Network, format!("offline or unreachable ({short})"))
        }
    }
}

/// `CSWAP_OFFLINE=1` keeps cswap off the network altogether: every fetch
/// fails at once and the stored numbers are shown. The tests run this way.
pub fn offline() -> bool {
    std::env::var_os("CSWAP_OFFLINE").is_some_and(|v| !v.is_empty() && v != "0")
}

/// GET `url` with `headers` and parse the body as JSON.
pub fn get_json(url: &str, headers: &[(&str, &str)]) -> Fetch<Value> {
    if offline() {
        return Err(FetchError::new(
            Why::Network,
            "offline (CSWAP_OFFLINE is set)",
        ));
    }
    let mut req = agent().get(url).set("Accept", "application/json");
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let resp = req.call().map_err(explain)?;
    resp.into_json()
        .map_err(|e| FetchError::new(Why::Shape, format!("unreadable answer ({e})")))
}

/// The payload of a JWT, unverified. Used only to read claims a vendor put in
/// its own token (email, plan, expiry); nothing is trusted on the strength of
/// it.
pub fn jwt_claims(token: &str) -> Option<Value> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_jwt_claims_without_padding() {
        use base64::Engine;
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"email":"a@b.c","exp":1}"#);
        let claims = jwt_claims(&format!("h.{body}.sig")).unwrap();
        assert_eq!(claims["email"], "a@b.c");
        assert!(jwt_claims("not-a-jwt").is_none());
    }

    #[test]
    fn picks_the_vendor_message_out_of_a_body() {
        assert_eq!(
            api_message(r#"{"error":{"message":"nope"}}"#).as_deref(),
            Some("nope")
        );
        assert_eq!(
            api_message(r#"{"detail":"slow down"}"#).as_deref(),
            Some("slow down")
        );
        assert_eq!(api_message("  ").as_deref(), None);
    }
}
