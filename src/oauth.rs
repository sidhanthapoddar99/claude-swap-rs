//! Anthropic OAuth: token refresh + usage fetch.
//!
//! Exactly three endpoints, all first-party. No telemetry, ever.

use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use serde_json::{json, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const BETA_HEADER: &str = "oauth-2025-04-20";
pub const USER_AGENT: &str = concat!("claude-swap-rs/", env!("CARGO_PKG_VERSION"));

/// Refresh margin: rotate when the access token dies within 5 minutes.
pub const REFRESH_MARGIN_MS: i64 = 5 * 60 * 1000;

/// Shared HTTP client. Bounded on purpose: `cswap list` fans out one usage
/// call per account, so an unreachable or hanging endpoint must fail fast
/// instead of freezing the terminal. Also disables ureq's retries — a usage
/// number isn't worth waiting through a backoff for.
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .try_proxy_from_env(true)
        .build()
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis() as i64
}

pub fn access_token(creds: &Value) -> Option<&str> {
    creds.get("claudeAiOauth")?.get("accessToken")?.as_str()
}

/// Refresh `claudeAiOauth` in place when the access token expires within
/// `margin_ms`. Returns true when a refresh actually happened (caller must
/// persist the mutated credentials).
pub fn refresh_if_needed(creds: &mut Value, margin_ms: i64) -> Result<bool> {
    let oauth = creds
        .get("claudeAiOauth")
        .and_then(Value::as_object)
        .context("credentials file has no claudeAiOauth object")?;
    let expires = oauth.get("expiresAt").and_then(Value::as_i64).unwrap_or(0);
    if expires - now_ms() > margin_ms {
        return Ok(false);
    }
    let refresh_token = oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .context("access token expired and no refreshToken present — re-login this account")?
        .to_string();

    let resp: Value = agent()
        .post(TOKEN_URL)
        .set("Content-Type", "application/json")
        .set("User-Agent", USER_AGENT)
        .send_json(json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLIENT_ID,
        }))
        .context("OAuth token refresh failed (network or server rejected the grant)")?
        .into_json()
        .context("OAuth token refresh: unparseable response")?;

    let access = resp
        .get("access_token")
        .and_then(Value::as_str)
        .context("refresh response missing access_token")?
        .to_string();
    let expires_in = resp
        .get("expires_in")
        .and_then(Value::as_i64)
        .unwrap_or(3600);

    let oauth = creds
        .get_mut("claudeAiOauth")
        .and_then(Value::as_object_mut)
        .expect("checked above");
    oauth.insert("accessToken".into(), json!(access));
    oauth.insert("expiresAt".into(), json!(now_ms() + expires_in * 1000));
    if let Some(rt) = resp.get("refresh_token").and_then(Value::as_str) {
        oauth.insert("refreshToken".into(), json!(rt));
    }
    if let Some(scope) = resp.get("scope").and_then(Value::as_str) {
        oauth.insert(
            "scopes".into(),
            json!(scope.split_whitespace().collect::<Vec<_>>()),
        );
    }
    Ok(true)
}

pub fn fetch_usage(token: &str) -> Result<Value> {
    // CSWAP_USAGE_URL redirects the usage call — a debugging/test hook, never
    // a default. Token refresh always goes to the real endpoint.
    let url = std::env::var("CSWAP_USAGE_URL").unwrap_or_else(|_| USAGE_URL.to_string());
    agent()
        .get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", BETA_HEADER)
        .set("User-Agent", USER_AGENT)
        .call()
        .context("usage API request failed")?
        .into_json()
        .context("usage API: unparseable response")
}

pub struct Window {
    pub label: String,
    pub pct: f64,
    pub resets_at: Option<String>,
}

/// Flatten the usage response into displayable windows:
/// the always-present 5h/7d gates plus any per-model weekly scoped limits.
pub fn windows(usage: &Value) -> Vec<Window> {
    let mut out = Vec::new();
    for (key, label) in [("five_hour", "5h"), ("seven_day", "7d")] {
        if let Some(w) = usage.get(key).and_then(Value::as_object) {
            if let Some(pct) = w.get("utilization").and_then(Value::as_f64) {
                out.push(Window {
                    label: label.to_string(),
                    pct,
                    resets_at: w.get("resets_at").and_then(Value::as_str).map(String::from),
                });
            }
        }
    }
    if let Some(limits) = usage.get("limits").and_then(Value::as_array) {
        for lim in limits {
            let name = lim
                .get("scope")
                .and_then(|s| s.get("model"))
                .and_then(|m| m.get("display_name"))
                .and_then(Value::as_str);
            let pct = lim.get("percent").and_then(Value::as_f64);
            if let (Some(name), Some(pct)) = (name, pct) {
                out.push(Window {
                    label: name.to_string(),
                    pct,
                    resets_at: lim
                        .get("resets_at")
                        .and_then(Value::as_str)
                        .map(String::from),
                });
            }
        }
    }
    out
}

/// "resets 2h 47m · 20:39" — the detailed form used by `usage`/`watch`.
/// Two units at most, coarsest first: `5d 3h`, `2h 47m`, `47m`.
pub fn reset_detail(resets_at: &str) -> Option<String> {
    let dt = DateTime::parse_from_rfc3339(resets_at).ok()?;
    let mins = (dt.with_timezone(&Utc) - Utc::now()).num_minutes().max(0);
    let clock = dt.with_timezone(&Local).format("%H:%M");
    Some(format!("resets {} · {clock}", humanize(mins)))
}

fn humanize(mins: i64) -> String {
    let (d, h, m) = (mins / 1440, (mins % 1440) / 60, mins % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_parses_gates_and_scoped() {
        let usage = json!({
            "five_hour": {"utilization": 34.5, "resets_at": "2026-07-21T16:00:00+00:00"},
            "seven_day": {"utilization": 61.0},
            "limits": [
                {"scope": {"model": {"display_name": "Fable"}}, "percent": 12.0,
                 "resets_at": "2026-07-24T00:00:00+00:00"},
                {"scope": {}, "percent": 5.0},
                "garbage"
            ]
        });
        let w = windows(&usage);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].label, "5h");
        assert!((w[0].pct - 34.5).abs() < f64::EPSILON);
        assert!(w[0].resets_at.is_some());
        assert_eq!(w[1].label, "7d");
        assert!(w[1].resets_at.is_none());
        assert_eq!(w[2].label, "Fable");
    }

    #[test]
    fn windows_empty_on_junk() {
        assert!(windows(&json!({})).is_empty());
        assert!(windows(&json!({"five_hour": "nope"})).is_empty());
    }

    #[test]
    fn refresh_skipped_when_fresh() {
        let mut creds = json!({"claudeAiOauth": {
            "accessToken": "tok", "refreshToken": "r",
            "expiresAt": now_ms() + 3_600_000
        }});
        // Fresh token: returns false without any network call.
        assert!(!refresh_if_needed(&mut creds, REFRESH_MARGIN_MS).unwrap());
    }

    #[test]
    fn refresh_errors_without_oauth_object() {
        let mut creds = json!({"mcpOAuth": {}});
        assert!(refresh_if_needed(&mut creds, 0).is_err());
    }

    #[test]
    fn reset_detail_formats_two_units() {
        let s = reset_detail("2999-01-01T00:00:00+00:00").unwrap();
        assert!(s.starts_with("resets ") && s.contains(" · "), "got {s}");
        assert!(reset_detail("garbage").is_none());
        assert_eq!(humanize(0), "0m");
        assert_eq!(humanize(47), "47m");
        assert_eq!(humanize(167), "2h 47m");
        assert_eq!(humanize(7380), "5d 3h");
    }
}
