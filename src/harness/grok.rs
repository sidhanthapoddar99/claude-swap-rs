//! xAI Grok Build (`grok`).
//!
//! Verified against grok 1.0.77: `GROK_HOME` moves the whole config dir, the
//! login is one entry in `auth.json`, and usage comes from the billing route
//! the CLI's own `/usage` reads. There is one window per account (weekly on
//! SuperGrok, monthly on some plans) and no session window.
//!
//! cswap never refreshes a Grok token. Access tokens last six hours and the
//! refresh token rotates under a lock protocol that is the CLI's own; an
//! account that has not run for a while reads "token expired" until `grok`
//! runs on it once.

use serde_json::Value;
use std::path::Path;

use super::{expired, pretty_plan, rfc3339, Fetched, Local, Spec};
use crate::fsx;
use crate::http::{self, Fetch, FetchError, Why};
use crate::model::{Credits, Snapshot, Window, WindowKind};
use crate::store::Identity;

pub const SPEC: Spec = Spec {
    id: "grok",
    name: "Grok Build",
    bins: &["grok"],
    home_env: Some("GROK_HOME"),
    home_flag: None,
    nested: &[],
    experimental: false,
    native: ".grok",
    private: &[
        "auth.json",
        "mcp_credentials.json",
        "settings_cache.json",
        "models_cache.json",
        "experiments_state.json",
        "campaigns_state.json",
        "managed_config.toml",
        "requirements.toml",
        "leader.sock",
        "logs",
        "memtrace",
        "crash",
    ],
    cred_file: "auth.json",
    login_life: "Access token lasts 6 hours and grok renews it by itself. xAI publishes no fixed lifetime for the login; it holds until you sign out or it is revoked.",
    scrub_env: &[
        "XAI_API_KEY",
        "GROK_CODE_XAI_API_KEY",
        "GROK_AUTH",
        "GROK_AUTH_PATH",
    ],
    login_args: &["login"],
    rgb: (176, 184, 201),
};

const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const FALLBACK_VERSION: &str = "1.0.77";

/// The one login entry in `auth.json`. The file is keyed by
/// `<issuer>::<client id>`; an account dir holds a single entry.
fn entry(config_dir: &Path) -> Option<Value> {
    let auth = fsx::read_value(&config_dir.join("auth.json"))?;
    auth.as_object()?
        .values()
        .find(|v| v.get("key").and_then(Value::as_str).is_some())
        .cloned()
}

pub fn logged_in(config_dir: &Path) -> bool {
    entry(config_dir).is_some()
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

pub fn local(config_dir: &Path) -> Local {
    let Some(e) = entry(config_dir) else {
        return Local::default();
    };
    // The plan name is cached from the CLI's last settings call. The payload
    // is a JSON string inside the JSON file.
    let plan = fsx::read_value(&config_dir.join("settings_cache.json"))
        .and_then(|c| {
            let payload = c.get("payload")?;
            match payload {
                Value::String(s) => serde_json::from_str::<Value>(s).ok(),
                other => Some(other.clone()),
            }
        })
        .and_then(|p| str_of(p.get("settings")?, "subscription_tier_display"));
    Local {
        identity: Identity {
            email: str_of(&e, "email"),
            account_id: str_of(&e, "user_id"),
            org: str_of(&e, "team_name").or_else(|| str_of(&e, "organization_name")),
            name: str_of(&e, "first_name"),
        },
        plan,
        token_expires: str_of(&e, "expires_at").as_deref().and_then(rfc3339),
        login_expires: None,
        billing: None,
    }
}

fn client_version(config_dir: &Path) -> String {
    fsx::read_value(&config_dir.join("version.json"))
        .and_then(|v| str_of(&v, "version"))
        .unwrap_or_else(|| FALLBACK_VERSION.to_string())
}

pub fn fetch(config_dir: &Path) -> Fetch<Fetched> {
    let e = entry(config_dir).ok_or_else(|| FetchError::new(Why::NoLogin, "not logged in"))?;
    let mut local = local(config_dir);
    if expired(local.token_expires) {
        return Err(FetchError::new(
            Why::Expired,
            "token expired: run grok once on this account to renew it",
        ));
    }
    let key = str_of(&e, "key").expect("entry() checked it");
    let url = std::env::var("CSWAP_GROK_USAGE_URL").unwrap_or_else(|_| BILLING_URL.to_string());
    let body = http::get_json(
        &url,
        &[
            ("Authorization", &format!("Bearer {key}")),
            ("x-grok-client-version", &client_version(config_dir)),
        ],
    )?;
    let (snapshot, plan) = parse(&body)?;
    if plan.is_some() {
        local.plan = plan;
    }
    Ok(Fetched { snapshot, local })
}

/// Read a key in either camelCase or snake_case: the CLI logs one and its
/// serde names say the other.
fn either<'a>(v: &'a Value, camel: &str, snake: &str) -> Option<&'a Value> {
    v.get(camel).or_else(|| v.get(snake))
}

/// `{"val": n}` or a bare number.
fn val(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    v.get("val").and_then(Value::as_f64).or_else(|| v.as_f64())
}

pub(crate) fn parse(body: &Value) -> Fetch<(Snapshot, Option<String>)> {
    let config = body
        .get("config")
        .filter(|c| c.is_object())
        .ok_or_else(|| FetchError::new(Why::Shape, "billing answer has no config"))?;

    let mut windows = Vec::new();
    if let Some(period) = either(config, "currentPeriod", "current_period") {
        let start = str_of(period, "start").as_deref().and_then(rfc3339);
        let end = str_of(period, "end").as_deref().and_then(rfc3339);
        let secs = match (start, end) {
            (Some(s), Some(e)) => Some((e - s).num_seconds()),
            _ => None,
        };
        let kind_text = str_of(period, "type").unwrap_or_default();
        let (kind, label) = if kind_text.contains("MONTH") {
            (WindowKind::Monthly, "month")
        } else if kind_text.contains("WEEK") {
            (WindowKind::Weekly, "week")
        } else {
            match secs.map(WindowKind::from_secs) {
                Some(WindowKind::Monthly) => (WindowKind::Monthly, "month"),
                _ => (WindowKind::Weekly, "week"),
            }
        };
        // Zero values are left out of the answer, so a missing percent is 0.
        let used = either(config, "creditUsagePercent", "credit_usage_percent")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        windows.push(Window {
            kind,
            label: label.to_string(),
            used_pct: used,
            resets_at: end,
            window_secs: secs,
        });
    }

    let balance = val(either(config, "prepaidBalance", "prepaid_balance"));
    let cap = val(either(config, "onDemandCap", "on_demand_cap"));
    let used = val(either(config, "onDemandUsed", "on_demand_used"));
    let on_demand = either(body, "onDemandEnabled", "on_demand_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let has_money = [balance, cap, used]
        .iter()
        .any(|v| v.is_some_and(|n| n > 0.0));
    let credits = (on_demand || has_money).then(|| Credits {
        enabled: on_demand || cap.is_some_and(|c| c > 0.0),
        used,
        limit: cap.filter(|c| *c > 0.0),
        balance,
        unit: "credits".to_string(),
        resets_at: None,
    });

    let plan = either(body, "subscriptionTier", "subscription_tier")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(pretty_plan);

    let mut snapshot = Snapshot::new(windows);
    snapshot.credits = credits;
    Ok((snapshot, plan))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_the_weekly_period_and_plan() {
        let body = json!({
            "config": {
                "creditUsagePercent": 10.0,
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "start": "2026-11-03T08:33:35.220671+00:00",
                    "end": "2026-11-10T08:33:35.220671+00:00"
                },
                "onDemandCap": {"val": 0},
                "prepaidBalance": {"val": 0}
            },
            "subscriptionTier": "SuperGrok"
        });
        let (snap, plan) = parse(&body).unwrap();
        assert_eq!(plan.as_deref(), Some("SuperGrok"));
        let w = snap.weekly().unwrap();
        assert_eq!(w.kind, WindowKind::Weekly);
        assert_eq!(w.used_pct, 10.0);
        assert_eq!(w.window_secs, Some(604800));
        assert!(snap.credits.is_none(), "all-zero money is not worth a row");
    }

    #[test]
    fn a_missing_percent_is_zero_and_snake_case_works() {
        let body = json!({
            "config": {
                "current_period": {
                    "type": "USAGE_PERIOD_TYPE_MONTHLY",
                    "start": "2026-11-01T00:00:00Z",
                    "end": "2026-12-01T00:00:00Z"
                },
                "prepaid_balance": {"val": 500}
            },
            "subscription_tier": "x_premium_plus"
        });
        let (snap, plan) = parse(&body).unwrap();
        assert_eq!(plan.as_deref(), Some("X Premium Plus"));
        let w = &snap.windows[0];
        assert_eq!((w.kind, w.used_pct), (WindowKind::Monthly, 0.0));
        assert_eq!(snap.credits.unwrap().balance, Some(500.0));
    }

    #[test]
    fn an_answer_without_config_is_a_shape_error() {
        assert_eq!(parse(&json!({"error": "x"})).unwrap_err().why, Why::Shape);
    }
}
