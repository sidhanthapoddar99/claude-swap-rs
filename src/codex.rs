//! Codex (OpenAI) usage — read-only, optional, and last.
//!
//! Codex keeps its login in `~/.codex/auth.json`. cswap reads that file and
//! nothing else: it never writes it, and it never refreshes the token. That is
//! the same contract `default` gets for `~/.claude`, for the same reason. Two
//! processes refreshing one token family means whichever ran last leaves the
//! other holding a dead ancestor, and cswap is not the tool that should be
//! rotating another vendor's credentials.
//!
//! Everything here is best-effort. No Codex install, no login, an expired
//! token, no network — every one of those renders nothing at all rather than an
//! error. A cswap user who does not use Codex should never learn that this
//! module exists.
//!
//! Note this reaches a THIRD endpoint, and the first one that is not
//! Anthropic's: `chatgpt.com/backend-api/codex/usage`. It is only ever called
//! when `~/.codex/auth.json` already exists.

use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;

use crate::oauth::{self, Window};
use crate::paths;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/codex/usage";
/// Codex CLI identifies itself this way; the endpoint is picky about it.
const ORIGINATOR: &str = "codex_cli_rs";

/// What one usage call returns: who it is, their plan, and every window.
pub struct Usage {
    pub email: String,
    pub plan: Option<String>,
    pub windows: Vec<Window>,
}

/// Is Codex even installed and logged in? Cheap and local — no network, so a
/// caller can skip the whole section without paying for a request.
pub fn is_configured() -> bool {
    paths::codex_auth().is_file()
}

/// Fetch the default Codex account and its windows.
///
/// The account comes back in the response, so there is no need to decode the
/// stored id_token to find out who this is.
pub fn usage() -> Result<Usage> {
    let text = fs::read_to_string(paths::codex_auth()).context("no ~/.codex/auth.json")?;
    let auth: Value = serde_json::from_str(&text).context("malformed ~/.codex/auth.json")?;
    let tokens = auth
        .get("tokens")
        .context("no ChatGPT login in auth.json")?;
    let access = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .context("no access token — run `codex login`")?;
    let account = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .unwrap_or_default();

    // CSWAP_CODEX_USAGE_URL redirects the call for tests. Never a default.
    let url = std::env::var("CSWAP_CODEX_USAGE_URL").unwrap_or_else(|_| USAGE_URL.to_string());
    let resp = oauth::agent()
        .get(&url)
        .set("Authorization", &format!("Bearer {access}"))
        .set("chatgpt-account-id", account)
        .set("originator", ORIGINATOR)
        .set("User-Agent", oauth::USER_AGENT)
        .set("Accept", "application/json")
        .call();
    let body: Value = match resp {
        Ok(r) => r.into_json().context("codex usage: unparseable response")?,
        // A 401 here means Codex's own token has expired. cswap will not
        // refresh it — running codex once does that, and does it correctly.
        Err(ureq::Error::Status(401, _)) => {
            anyhow::bail!("codex token expired — run `codex` once to refresh it")
        }
        Err(e) => return Err(anyhow::anyhow!("{e}")).context("codex usage request failed"),
    };

    Ok(Usage {
        email: body
            .get("email")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        plan: body
            .get("plan_type")
            .and_then(Value::as_str)
            .map(String::from),
        windows: windows(&body),
    })
}

/// Flatten the response into displayable windows: the account's own primary and
/// secondary gates first, then any per-model limit.
fn windows(body: &Value) -> Vec<Window> {
    let mut out = Vec::new();
    if let Some(rl) = body.get("rate_limit") {
        push_pair(&mut out, rl, None);
    }
    if let Some(extra) = body.get("additional_rate_limits").and_then(Value::as_array) {
        for lim in extra {
            let name = lim.get("limit_name").and_then(Value::as_str);
            if let Some(rl) = lim.get("rate_limit") {
                push_pair(&mut out, rl, name);
            }
        }
    }
    out
}

/// Both windows of one rate_limit object. `name` overrides the duration label,
/// which is what distinguishes a per-model limit from an account-wide gate.
fn push_pair(out: &mut Vec<Window>, rl: &Value, name: Option<&str>) {
    for key in ["primary_window", "secondary_window"] {
        let Some(w) = rl.get(key).and_then(Value::as_object) else {
            continue; // null when the plan has no window of that kind
        };
        let Some(pct) = w.get("used_percent").and_then(Value::as_f64) else {
            continue;
        };
        let label = match name {
            Some(n) => n.to_string(),
            None => w
                .get("limit_window_seconds")
                .and_then(Value::as_i64)
                .map(window_label)
                .unwrap_or_else(|| "limit".to_string()),
        };
        out.push(Window {
            label,
            pct,
            resets_at: w.get("reset_at").and_then(Value::as_i64).and_then(rfc3339),
        });
    }
}

/// 18000 -> "5h", 604800 -> "7d". cswap's other labels read this way, so a
/// Codex row lines up with a Claude row without explanation.
fn window_label(seconds: i64) -> String {
    let mins = seconds / 60;
    if mins % 1440 == 0 && mins >= 1440 {
        format!("{}d", mins / 1440)
    } else if mins % 60 == 0 && mins >= 60 {
        format!("{}h", mins / 60)
    } else {
        format!("{mins}m")
    }
}

/// Codex reports resets as unix seconds; `oauth::reset_detail` reads RFC 3339.
fn rfc3339(unix_seconds: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(unix_seconds, 0).map(|dt| dt.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "email": "dev@x.com",
            "plan_type": "prolite",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 53, "limit_window_seconds": 604800,
                    "reset_at": 1787213881i64
                },
                "secondary_window": null
            },
            "additional_rate_limits": [{
                "limit_name": "GPT-5.3-Codex-Spark",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 0, "limit_window_seconds": 604800,
                        "reset_at": 1787615516i64
                    },
                    "secondary_window": null
                }
            }]
        })
    }

    #[test]
    fn flattens_account_gates_then_per_model_limits() {
        let w = windows(&sample());
        assert_eq!(w.len(), 2, "a null secondary_window contributes nothing");
        assert_eq!(w[0].label, "7d");
        assert!((w[0].pct - 53.0).abs() < f64::EPSILON);
        assert!(w[0].resets_at.as_deref().unwrap().starts_with("2026-"));
        assert_eq!(w[1].label, "GPT-5.3-Codex-Spark", "named limits keep names");
    }

    #[test]
    fn window_labels_match_the_claude_side() {
        assert_eq!(window_label(604800), "7d");
        assert_eq!(window_label(18000), "5h");
        assert_eq!(window_label(300), "5m");
    }

    #[test]
    fn junk_yields_no_windows_instead_of_panicking() {
        assert!(windows(&json!({})).is_empty());
        assert!(windows(&json!({"rate_limit": "nope"})).is_empty());
        assert!(windows(&json!({"rate_limit": {"primary_window": {}}})).is_empty());
    }
}
