//! OpenAI Codex CLI (`codex`).
//!
//! Verified against codex-cli 0.167.0 and its source at the same tag:
//! `CODEX_HOME` moves the config dir, `CODEX_SQLITE_HOME` keeps every account
//! on the one set of databases, the login is `auth.json`, and usage comes from
//! `wham/usage`, the route the CLI's own status view polls.
//!
//! Codex also has real "resets": rate-limit reset credits, each with its own
//! expiry. cswap lists them. It never calls the route that spends one.
//!
//! cswap never refreshes a Codex token. The access token lasts ten days and the
//! refresh token is single use, so a second refresher would log the account
//! out. An account idle for longer reads "token expired" until codex runs once.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use std::path::Path;

use super::{expired, pretty_plan, rfc3339, unix, Fetched, Local, Spec};
use crate::fsx;
use crate::http::{self, Fetch, FetchError, Why};
use crate::model::{Credits, Grant, Snapshot, Source, Window, WindowKind};
use crate::store::{Billing, Cycle, Identity};

pub const SPEC: Spec = Spec {
    id: "codex",
    name: "Codex",
    bins: &["codex"],
    home_env: Some("CODEX_HOME"),
    home_flag: None,
    nested: &[],
    experimental: false,
    native: ".codex",
    private: &[
        // The login and its write lock.
        "auth.json",
        "auth.lock",
        ".credentials.json",
        // Sockets, pids and scratch of the processes of one home. A shared
        // control socket would attach one account's TUI to another's daemon.
        "app-server-control",
        "app-server-daemon",
        "ipc",
        "node_repl",
        "shell_snapshots",
        "tmp",
        ".tmp",
        "project-metadata-locks",
        // Caches keyed by who is logged in.
        "models_cache.json",
        "cache",
        "log",
        // The installed binary. Codex compares its own path against the home,
        // so a link here would not be recognised anyway. `codex update`
        // therefore runs on the root home: see `own_args`.
        "packages",
        // The desktop app's own state.
        "sqlite",
        "chrome-native-hosts-v2.json",
        // Databases are shared through CODEX_SQLITE_HOME, never by link.
        "*.sqlite",
        "*.sqlite-wal",
        "*.sqlite-shm",
        ".sqlite-maintenance.lock",
        ".codex-global-state.json*",
        "config.toml.bak",
        "config.toml.backup-*",
    ],
    cred_file: "auth.json",
    login_life: "Access token lasts 10 days and codex renews it by itself. OpenAI publishes no fixed lifetime for the login; it ends on sign-out, a password change or revocation.",
    scrub_env: &["CODEX_API_KEY", "CODEX_ACCESS_TOKEN"],
    own_args: &["update"],
    login_args: &["login"],
    rgb: (16, 163, 127),
};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const RESETS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const ORIGINATOR: &str = "codex_cli_rs";
const AUTH_CLAIMS: &str = "https://api.openai.com/auth";

fn auth(config_dir: &Path) -> Option<Value> {
    fsx::read_value(&config_dir.join("auth.json"))
}

fn token<'a>(auth: &'a Value, name: &str) -> Option<&'a str> {
    auth.get("tokens")?
        .get(name)?
        .as_str()
        .filter(|s| !s.is_empty())
}

pub fn logged_in(config_dir: &Path) -> bool {
    auth(config_dir).is_some_and(|a| {
        token(&a, "access_token").is_some()
            || a.get("OPENAI_API_KEY").and_then(Value::as_str).is_some()
    })
}

/// Does the shared config send credentials to the OS keyring? Then there is no
/// `auth.json` for cswap to read.
fn uses_keyring(config_dir: &Path) -> bool {
    std::fs::read_to_string(config_dir.join("config.toml"))
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|t| {
            t.get("cli_auth_credentials_store")
                .and_then(|v| v.as_str().map(String::from))
        })
        .is_some_and(|mode| mode == "keyring" || mode == "auto")
}

/// Codex shows its plans under different names than the API uses.
fn plan_name(raw: &str) -> String {
    match raw {
        "prolite" => "Pro".to_string(),
        "pro" => "Pro (More)".to_string(),
        "promax" => "Pro (Max)".to_string(),
        "hc" => "Enterprise".to_string(),
        other => pretty_plan(other),
    }
}

/// The subscription dates OpenAI puts in the id token. Codex itself never
/// reads them; they are as fresh as the last token refresh.
fn billing_from(claims: &Value) -> Option<Billing> {
    let until = claims
        .get("chatgpt_subscription_active_until")
        .and_then(Value::as_str)
        .and_then(rfc3339)?;
    let start = claims
        .get("chatgpt_subscription_active_start")
        .and_then(Value::as_str)
        .and_then(rfc3339);
    let cycle = start.and_then(|s| match (until - s).num_days() {
        25..=35 => Some(Cycle::Monthly),
        350..=380 => Some(Cycle::Yearly),
        _ => None,
    });
    let local: NaiveDate = until.with_timezone(&chrono::Local).date_naive();
    Some(Billing {
        cycle,
        renews_on: Some(local),
        cancelled: false,
        source: Source::Api,
    })
}

pub fn local(config_dir: &Path) -> Local {
    let Some(a) = auth(config_dir) else {
        return Local::default();
    };
    let id = token(&a, "id_token").and_then(http::jwt_claims);
    let access = token(&a, "access_token").and_then(http::jwt_claims);
    let claims = id
        .as_ref()
        .and_then(|c| c.get(AUTH_CLAIMS))
        .or_else(|| access.as_ref().and_then(|c| c.get(AUTH_CLAIMS)));
    let s = |v: Option<&Value>| v.and_then(Value::as_str).map(String::from);

    let email = s(id.as_ref().and_then(|c| c.get("email"))).or_else(|| {
        s(access
            .as_ref()
            .and_then(|c| c.pointer("/https:~1~1api.openai.com~1profile/email")))
    });
    let org = claims
        .and_then(|c| c.get("organizations"))
        .and_then(Value::as_array)
        .and_then(|orgs| {
            orgs.iter()
                .find(|o| o.get("is_default").and_then(Value::as_bool) == Some(true))
                .or_else(|| orgs.first())
        })
        .and_then(|o| s(o.get("title")))
        .filter(|t| !t.is_empty() && t != "Personal");
    Local {
        identity: Identity {
            email,
            account_id: token(&a, "account_id")
                .map(String::from)
                .or_else(|| s(claims.and_then(|c| c.get("chatgpt_account_id")))),
            org,
            name: s(id.as_ref().and_then(|c| c.get("name"))),
        },
        plan: s(claims.and_then(|c| c.get("chatgpt_plan_type"))).map(|p| plan_name(&p)),
        token_expires: access
            .as_ref()
            .and_then(|c| c.get("exp"))
            .and_then(Value::as_i64)
            .and_then(unix),
        login_expires: None,
        billing: claims.and_then(billing_from),
    }
}

pub fn fetch(config_dir: &Path) -> Fetch<Fetched> {
    let Some(a) = auth(config_dir) else {
        let text = if uses_keyring(config_dir) {
            "login is in the OS keyring (cli_auth_credentials_store), which cswap cannot read"
        } else {
            "not logged in"
        };
        return Err(FetchError::new(Why::NoLogin, text));
    };
    let Some(access) = token(&a, "access_token") else {
        return Err(FetchError::new(
            Why::Unsupported,
            "API-key login: usage limits apply to ChatGPT logins only",
        ));
    };
    let mut local = local(config_dir);
    if expired(local.token_expires) {
        return Err(FetchError::new(
            Why::Expired,
            "token expired: run codex once on this account to renew it",
        ));
    }
    let account = token(&a, "account_id").unwrap_or_default();
    let bearer = format!("Bearer {access}");
    let headers = [
        ("Authorization", bearer.as_str()),
        ("chatgpt-account-id", account),
        ("originator", ORIGINATOR),
    ];

    let url = std::env::var("CSWAP_CODEX_USAGE_URL").unwrap_or_else(|_| USAGE_URL.to_string());
    let body = http::get_json(&url, &headers)?;
    let mut snapshot = parse(&body);

    // The usage answer only counts the reset credits. The list, with each
    // one's expiry, is a second read, made only when there is something to
    // list. A failure here must not cost the usage that was just fetched.
    let available = body
        .pointer("/rate_limit_reset_credits/available_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if available > 0 {
        let url =
            std::env::var("CSWAP_CODEX_RESETS_URL").unwrap_or_else(|_| RESETS_URL.to_string());
        snapshot.grants_at = Some(Utc::now());
        snapshot.grants = match http::get_json(&url, &headers) {
            Ok(list) => parse_grants(&list, Utc::now()),
            Err(_) => vec![Grant {
                scope: "all limits".to_string(),
                count: available as u32,
                expires_at: None,
                note: "expiry unknown: the list could not be read".to_string(),
                source: Source::Api,
                usable_now: None,
            }],
        };
    }

    if let Some(email) = body.get("email").and_then(Value::as_str) {
        local.identity.email = Some(email.to_string());
    }
    if let Some(plan) = body.get("plan_type").and_then(Value::as_str) {
        local.plan = Some(plan_name(plan));
    }
    Ok(Fetched { snapshot, local })
}

/// `18000` -> `5h`, `604800` -> `week`.
fn window_label(kind: WindowKind, secs: i64) -> String {
    match kind {
        WindowKind::Weekly => "week".to_string(),
        WindowKind::Monthly => "month".to_string(),
        WindowKind::Daily => "day".to_string(),
        _ if secs % 3600 == 0 && secs >= 3600 => format!("{}h", secs / 3600),
        _ => format!("{}m", (secs + 59) / 60),
    }
}

/// Both windows of one `rate_limit` object. Which one is the weekly window
/// differs by plan, so each is classified by its own length. `name` marks a
/// limit that covers one model or feature rather than the whole account.
fn push_pair(out: &mut Vec<Window>, rl: &Value, name: Option<&str>) {
    for key in ["primary_window", "secondary_window"] {
        let Some(w) = rl.get(key).filter(|w| w.is_object()) else {
            continue;
        };
        let Some(pct) = w.get("used_percent").and_then(Value::as_f64) else {
            continue;
        };
        let secs = w
            .get("limit_window_seconds")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let base = WindowKind::from_secs(secs);
        let (kind, label) = match name {
            None => (base, window_label(base, secs)),
            Some(n) if base == WindowKind::Weekly => (WindowKind::WeeklyScoped, n.to_string()),
            Some(n) => (
                WindowKind::Other,
                format!("{n} {}", window_label(base, secs)),
            ),
        };
        out.push(Window {
            kind,
            label,
            used_pct: pct,
            resets_at: w.get("reset_at").and_then(Value::as_i64).and_then(unix),
            window_secs: (secs > 0).then_some(secs),
        });
    }
}

pub(crate) fn parse(body: &Value) -> Snapshot {
    let mut windows = Vec::new();
    if let Some(rl) = body.get("rate_limit") {
        push_pair(&mut windows, rl, None);
    }
    if let Some(extra) = body.get("additional_rate_limits").and_then(Value::as_array) {
        for lim in extra {
            let name = lim
                .get("limit_name")
                .and_then(Value::as_str)
                .unwrap_or("extra limit");
            if let Some(rl) = lim.get("rate_limit") {
                push_pair(&mut windows, rl, Some(name));
            }
        }
    }
    if let Some(rl) = body.get("code_review_rate_limit").filter(|v| v.is_object()) {
        push_pair(&mut windows, rl, Some("code review"));
    }
    // Shortest account-wide window first, then the scoped ones.
    windows.sort_by_key(|w| {
        (
            matches!(w.kind, WindowKind::WeeklyScoped | WindowKind::Other),
            w.window_secs.unwrap_or(i64::MAX),
        )
    });

    let credits = body.get("credits").and_then(|c| {
        let has = c
            .get("has_credits")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let unlimited = c.get("unlimited").and_then(Value::as_bool).unwrap_or(false);
        // The balance is a decimal in a string.
        let balance = c.get("balance").and_then(|b| match b {
            Value::String(s) => s.parse::<f64>().ok(),
            other => other.as_f64(),
        });
        (has || unlimited || balance.is_some_and(|b| b > 0.0)).then(|| Credits {
            enabled: has || unlimited,
            used: None,
            limit: None,
            balance,
            unit: "credits".to_string(),
            resets_at: None,
        })
    });

    let mut snapshot = Snapshot::new(windows);
    snapshot.credits = credits;
    snapshot
}

/// The available reset credits, grouped by expiry so two grants that run out
/// together show as one row of two.
pub(crate) fn parse_grants(list: &Value, now: DateTime<Utc>) -> Vec<Grant> {
    let mut grants: Vec<Grant> = Vec::new();
    let Some(credits) = list.get("credits").and_then(Value::as_array) else {
        return grants;
    };
    for c in credits {
        if c.get("status").and_then(Value::as_str) != Some("available") {
            continue;
        }
        if c.get("is_supported_by_plan").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let expires_at = c
            .get("expires_at")
            .and_then(Value::as_str)
            .and_then(rfc3339);
        if expires_at.is_some_and(|t| t <= now) {
            continue;
        }
        let title = c
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Reset")
            .to_string();
        match grants
            .iter_mut()
            .find(|g| g.expires_at == expires_at && g.note == title)
        {
            Some(g) => g.count += 1,
            None => grants.push(Grant {
                scope: "all limits".to_string(),
                count: 1,
                expires_at,
                note: title,
                source: Source::Api,
                usable_now: None,
            }),
        }
    }
    grants.sort_by_key(|g| g.expires_at.unwrap_or(DateTime::<Utc>::MAX_UTC));
    grants
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_weekly_primary_window_is_the_weekly_window() {
        // The prolite plan: one window, weekly, in the "primary" slot.
        let body = json!({
            "plan_type": "prolite",
            "rate_limit": {
                "primary_window": {"used_percent": 88, "limit_window_seconds": 604800,
                                   "reset_at": 1794607688i64},
                "secondary_window": null
            },
            "additional_rate_limits": null,
            "credits": {"has_credits": false, "unlimited": false, "balance": "0"}
        });
        let snap = parse(&body);
        assert_eq!(snap.windows.len(), 1);
        let w = snap.weekly().unwrap();
        assert_eq!((w.label.as_str(), w.used_pct), ("week", 88.0));
        assert_eq!(w.resets_at.unwrap().timestamp(), 1794607688);
        assert!(snap.session().is_none());
        assert!(snap.credits.is_none());
    }

    #[test]
    fn five_hour_plus_weekly_and_scoped_limits() {
        let body = json!({
            "rate_limit": {
                "primary_window": {"used_percent": 12, "limit_window_seconds": 18000,
                                   "reset_at": 1794000000i64},
                "secondary_window": {"used_percent": 40, "limit_window_seconds": 604800,
                                     "reset_at": 1794600000i64}
            },
            "additional_rate_limits": [{
                "limit_name": "GPT-5.3-Codex-Spark",
                "rate_limit": {"primary_window": {"used_percent": 3,
                    "limit_window_seconds": 604800, "reset_at": 1794600000i64}}
            }],
            "credits": {"has_credits": true, "balance": "12.5"}
        });
        let snap = parse(&body);
        assert_eq!(snap.session().unwrap().label, "5h");
        assert_eq!(snap.weekly().unwrap().used_pct, 40.0);
        let scoped = &snap.windows[2];
        assert_eq!(scoped.kind, WindowKind::WeeklyScoped);
        assert_eq!(scoped.label, "GPT-5.3-Codex-Spark");
        assert_eq!(snap.credits.unwrap().balance, Some(12.5));
    }

    #[test]
    fn junk_yields_no_windows() {
        assert!(parse(&json!({})).windows.is_empty());
        assert!(parse(&json!({"rate_limit": "nope"})).windows.is_empty());
        assert!(parse(&json!({"rate_limit": {"primary_window": {}}}))
            .windows
            .is_empty());
    }

    #[test]
    fn reset_credits_group_by_expiry_and_skip_spent_ones() {
        let now = Utc::now();
        let soon = (now + chrono::Duration::days(3)).to_rfc3339();
        let list = json!({"credits": [
            {"status": "available", "expires_at": soon, "title": "Full reset"},
            {"status": "available", "expires_at": soon, "title": "Full reset"},
            {"status": "redeemed", "expires_at": soon, "title": "Full reset"},
            {"status": "available", "expires_at": "2020-01-01T00:00:00Z", "title": "Full reset"},
            {"status": "available", "expires_at": null, "title": "Bonus"}
        ]});
        let grants = parse_grants(&list, now);
        assert_eq!(grants.len(), 2);
        assert_eq!(
            (grants[0].count, grants[0].note.as_str()),
            (2, "Full reset")
        );
        assert!(grants[1].expires_at.is_none(), "no expiry sorts last");
    }

    #[test]
    fn subscription_dates_come_from_the_id_token_claims() {
        let b = billing_from(&json!({
            "chatgpt_subscription_active_start": "2026-10-22T08:27:47+00:00",
            "chatgpt_subscription_active_until": "2026-11-22T08:27:47+00:00"
        }))
        .unwrap();
        assert_eq!(b.cycle, Some(Cycle::Monthly));
        assert_eq!(b.source, Source::Api);
        assert!(billing_from(&json!({})).is_none());
    }

    #[test]
    fn codex_plan_names() {
        assert_eq!(plan_name("prolite"), "Pro");
        assert_eq!(plan_name("plus"), "Plus");
        assert_eq!(
            plan_name("self_serve_business_prolite"),
            "Self Serve Business Prolite"
        );
    }
}
