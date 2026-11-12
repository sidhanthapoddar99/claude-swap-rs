//! Anthropic Claude Code (`claude`).
//!
//! Verified against Claude Code 2.1.322: `CLAUDE_CONFIG_DIR` moves the config
//! dir (and `.claude.json` with it), the login is `.credentials.json`, and
//! usage comes from `/api/oauth/usage`, the route the CLI's own `/usage` reads.
//!
//! Three rules shape this module:
//!
//! - cswap never renews a Claude token. The refresh token is single use and
//!   the CLI guards it with its own lock, so a second refresher would log the
//!   account out. An account whose access token has run out keeps its last
//!   known numbers until `claude` next runs on it.
//! - Claude Code caches the usage answer in its own `.claude.json`. When that
//!   copy is fresh, cswap reads it and makes no request at all: the endpoint
//!   is rate limited per token, and a running session spends the same budget.
//! - Claude has real "resets": limit-reset grants, each with an expiry. The
//!   API lists them only to its own CLI, so that one read identifies as the
//!   CLI (`cli_identity` in the harness settings turns this off). cswap never
//!   calls the route that spends one.

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::{expired, pretty_plan, rfc3339, unix, Fetched, Harness, Local, Spec};
use crate::http::{self, Fetch, FetchError, Why};
use crate::model::{Credits, Grant, Snapshot, Source, Window, WindowKind};
use crate::store::{Billing, Cycle, HarnessSettings, Identity};
use crate::{fsx, paths};

pub const SPEC: Spec = Spec {
    id: "claude",
    name: "Claude Code",
    bins: &["claude"],
    home_env: Some("CLAUDE_CONFIG_DIR"),
    home_flag: None,
    nested: &[],
    experimental: false,
    native: ".claude",
    private: &[
        // The login. Claude refuses to read it through a link.
        ".credentials.json",
        // Identity, onboarding, per-project trust and per-account caches. With
        // a config dir set it lives inside the dir.
        ".claude.json",
        ".claude.json.backup",
        // A non-empty legacy config here would override `.claude.json`.
        ".config.json",
        // Snapshots of `.claude.json`: a shared copy would let one account
        // restore another's identity.
        "backups",
        // A version-controlled native dir must not make an account its work tree.
        ".git",
        // Locks around token renewal and credential writes.
        ".oauth_refresh.lock",
        ".storage-write",
        // Settings and limits an organisation pushes to its own members.
        "policy-limits.json",
        "remote-settings*",
        "mcp-needs-auth-cache.json",
        // One background daemon per config dir.
        "daemon",
        "daemon.log",
        "state",
        "stats-cache.json",
        ".last-cleanup",
        ".last-update-result.json",
    ],
    cred_file: ".credentials.json",
    login_life: "Access token lasts about 8 hours and claude renews it by itself. The login itself runs out about 30 days after its last use; any use extends it.",
    scrub_env: &[
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    ],
    own_args: &["update"],
    login_args: &["auth", "login"],
    rgb: (217, 119, 87),
};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const BETA: &str = "oauth-2025-04-20";
/// The reset grants change rarely (the CLI itself looks every three days).
const GRANTS_EVERY_HOURS: i64 = 6;
/// A usage answer Claude Code cached this recently is used instead of a call.
const DISK_FRESH_MINUTES: i64 = 8;
const FALLBACK_CLI_VERSION: &str = "2.1.322";

/// Keys copied once from the native `.claude.json` into a new account's:
/// user-scope MCP servers and per-project trust. Convenience only; none of it
/// is a credential, and no identity is copied.
const SEEDED_KEYS: &[&str] = &["mcpServers", "projects"];

/// Where the global config file is: next to the native dir, inside any other.
fn claude_json(config_dir: &Path, native: bool) -> PathBuf {
    if native {
        paths::home().join(".claude.json")
    } else {
        config_dir.join(".claude.json")
    }
}

/// The stored login. Linux keeps it in a plain file. macOS keeps it in the
/// keychain, under a service name derived from the config dir.
fn credentials(config_dir: &Path, native: bool) -> Option<Value> {
    if let Some(v) = fsx::read_value(&config_dir.join(".credentials.json")) {
        return Some(v);
    }
    keychain(config_dir, native)
}

#[cfg(target_os = "macos")]
fn keychain(config_dir: &Path, native: bool) -> Option<Value> {
    use sha2::{Digest, Sha256};
    let service = if native {
        "Claude Code-credentials".to_string()
    } else {
        let hash = Sha256::digest(config_dir.to_string_lossy().as_bytes());
        let hex: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();
        format!("Claude Code-credentials-{hex}")
    };
    let user = std::env::var("USER").unwrap_or_else(|_| "claude-code-user".to_string());
    let out = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-a", &user, "-w", "-s", &service])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

#[cfg(not(target_os = "macos"))]
fn keychain(_config_dir: &Path, _native: bool) -> Option<Value> {
    None
}

fn oauth(creds: &Value) -> Option<&Value> {
    creds.get("claudeAiOauth").filter(|o| {
        o.get("accessToken")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    })
}

pub fn logged_in(config_dir: &Path) -> bool {
    if fsx::read_value(&config_dir.join(".credentials.json"))
        .as_ref()
        .and_then(oauth)
        .is_some()
    {
        return true;
    }
    // On macOS the login is in the keychain. Asking the keychain can raise a
    // permission prompt, so this cheap check goes by the recorded identity and
    // leaves the keychain to the usage fetch.
    cfg!(target_os = "macos") && {
        let native = config_dir == Harness::Claude.native_dir();
        fsx::read_value(&claude_json(config_dir, native))
            .is_some_and(|c| c.get("oauthAccount").is_some_and(Value::is_object))
    }
}

fn s(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(String::from)
}

/// Anthropic's name for a rate-limit tier. Its two Max plans were renamed on
/// 30 October 2026: Max 5x is "Max" and Max 20x is "Ultra". The tiers kept
/// their old names.
fn plan_name(tier: &str) -> String {
    let known = tier.trim().to_lowercase();
    match known
        .trim_start_matches("default_")
        .trim_start_matches("claude_")
    {
        "max_5x" => "Max".to_string(),
        "max_20x" => "Ultra".to_string(),
        _ => pretty_plan(tier),
    }
}

pub fn local(config_dir: &Path, native: bool) -> Local {
    let config = fsx::read_value(&claude_json(config_dir, native));
    let account = config.as_ref().and_then(|c| c.get("oauthAccount"));
    let creds = fsx::read_value(&config_dir.join(".credentials.json"));
    let oauth = creds.as_ref().and_then(oauth);
    let get = |k: &str| s(account.and_then(|a| a.get(k)));

    // The plan as Anthropic names it, else worked out from the rate-limit
    // tier. The tier in the profile comes first: Claude Code fetches the
    // profile again and again, while the tier beside the token is written at
    // sign-in and can be a plan behind after an upgrade.
    let plan = get("planDisplayName")
        .or_else(|| {
            get("organizationRateLimitTier")
                .or_else(|| s(oauth.and_then(|o| o.get("rateLimitTier"))))
                .map(|t| plan_name(&t))
        })
        .or_else(|| s(oauth.and_then(|o| o.get("subscriptionType"))).map(|t| pretty_plan(&t)));
    let ms = |k: &str| {
        oauth
            .and_then(|o| o.get(k))
            .and_then(Value::as_i64)
            .and_then(unix)
    };
    Local {
        identity: Identity {
            email: get("emailAddress"),
            account_id: get("accountUuid"),
            org: get("organizationName").filter(|o| {
                // A personal plan's organisation is just "<email>'s Organization".
                !o.contains("'s Organi")
            }),
            name: get("displayName").or_else(|| get("fullName")),
        },
        plan,
        token_expires: ms("expiresAt"),
        login_expires: ms("refreshTokenExpiresAt"),
        billing: None,
    }
}

/// First-launch `.claude.json`: the two keys that skip onboarding (Claude
/// shows the wizard when `theme` or `hasCompletedOnboarding` is missing), plus
/// one-time copies of MCP servers and project trust from the native config.
pub fn seed(config_dir: &Path) -> anyhow::Result<()> {
    let path = config_dir.join(".claude.json");
    if path.exists() {
        return Ok(());
    }
    let live = fsx::read_value(&claude_json(&Harness::Claude.native_dir(), true))
        .unwrap_or_else(|| json!({}));
    let mut out = json!({ "hasCompletedOnboarding": true });
    out["theme"] = json!(live.get("theme").and_then(Value::as_str).unwrap_or("dark"));
    for key in SEEDED_KEYS {
        if let Some(v) = live.get(*key) {
            out[*key] = v.clone();
        }
    }
    fsx::write_json(&path, &out)
}

/// The usage answer Claude Code itself cached, when it belongs to the account
/// that is signed in now.
fn disk_cache(config_dir: &Path, native: bool) -> Option<(DateTime<Utc>, Value)> {
    let config = fsx::read_value(&claude_json(config_dir, native))?;
    let cache = config.get("cachedUsageUtilization")?;
    let owner = cache.get("accountUuid").and_then(Value::as_str);
    let current = config
        .pointer("/oauthAccount/accountUuid")
        .and_then(Value::as_str);
    if owner.is_some() && owner != current {
        return None;
    }
    let at = cache
        .get("fetchedAtMs")
        .and_then(Value::as_i64)
        .and_then(unix)?;
    let body = cache.get("utilization").filter(|u| u.is_object())?.clone();
    Some((at, body))
}

fn cli_version(native_dir: &Path) -> String {
    // The installed versions sit beside each other; the newest name wins.
    let versions = paths::home().join(".local/share/claude/versions");
    let _ = native_dir;
    std::fs::read_dir(versions)
        .ok()
        .and_then(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.split('.').all(|p| p.parse::<u32>().is_ok()))
                .max_by_key(|n| {
                    n.split('.')
                        .map(|p| p.parse::<u32>().unwrap_or(0))
                        .collect::<Vec<_>>()
                })
        })
        .unwrap_or_else(|| FALLBACK_CLI_VERSION.to_string())
}

pub fn fetch(config_dir: &Path, native: bool, prev: Option<&Snapshot>) -> Fetch<Fetched> {
    let creds = credentials(config_dir, native)
        .ok_or_else(|| FetchError::new(Why::NoLogin, "not signed in"))?;
    let Some(oauth) = oauth(&creds) else {
        return Err(FetchError::new(Why::NoLogin, "not signed in"));
    };
    let local = local(config_dir, native);
    let now = Utc::now();
    let cli_identity = HarnessSettings::load(Harness::Claude)
        .map(|h| h.cli_identity)
        .unwrap_or(true);
    let grants_stale = prev
        .and_then(|p| p.grants_at)
        .is_none_or(|t| now - t > Duration::hours(GRANTS_EVERY_HOURS));
    let want_grants = cli_identity && grants_stale;

    // Carry the last known grants into a snapshot that did not ask for them.
    let carry = |mut snap: Snapshot| -> Snapshot {
        if let Some(p) = prev {
            snap.grants = p
                .grants
                .iter()
                .filter(|g| !g.expired(now))
                .cloned()
                .collect();
            snap.grants_at = p.grants_at;
            if snap.renewal_hint.is_none() {
                snap.renewal_hint = p.renewal_hint.clone();
            }
        }
        snap
    };
    let from_disk = |min_age: Option<DateTime<Utc>>| -> Option<Snapshot> {
        let (at, body) = disk_cache(config_dir, native)?;
        if min_age.is_some_and(|m| at <= m) {
            return None;
        }
        let mut snap = parse(&body, now);
        snap.fetched_at = at;
        Some(carry(snap))
    };
    let newer_than_prev = prev.map(|p| p.fetched_at);
    let done = |snapshot: Snapshot| Fetched {
        local: with_billing(local.clone(), &snapshot),
        snapshot,
    };

    // Claude Code asked a moment ago: use its answer, spend no request.
    if !want_grants {
        if let Some(snap) = from_disk(Some(now - Duration::minutes(DISK_FRESH_MINUTES))) {
            return Ok(done(snap));
        }
    }

    let expires = oauth
        .get("expiresAt")
        .and_then(Value::as_i64)
        .and_then(unix);
    if expired(expires) {
        // The token has run out and only claude may renew it. Whatever claude
        // last cached is still the best there is.
        return match from_disk(newer_than_prev) {
            Some(snap) => Ok(done(snap)),
            None => Err(FetchError::new(
                Why::Expired,
                "token expired: claude renews it the next time it runs on this account",
            )),
        };
    }

    let token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .expect("oauth() checked it");
    let bearer = format!("Bearer {token}");
    let base = std::env::var("CSWAP_CLAUDE_USAGE_URL").unwrap_or_else(|_| USAGE_URL.to_string());
    let result = if want_grants {
        let agent = format!("claude-cli/{} (external, cli)", cli_version(config_dir));
        http::get_json(
            &format!("{base}?cedar_ember=1&skip_spend=1"),
            &[
                ("Authorization", &bearer),
                ("anthropic-beta", BETA),
                ("User-Agent", &agent),
                ("x-app", "cli"),
            ],
        )
    } else {
        http::get_json(
            &base,
            &[("Authorization", &bearer), ("anthropic-beta", BETA)],
        )
    };
    match result {
        Ok(body) => {
            let mut snap = parse(&body, now);
            if want_grants {
                snap.grants_at = Some(now);
            } else {
                snap = carry(snap);
            }
            Ok(done(snap))
        }
        Err(e) => match from_disk(newer_than_prev) {
            Some(snap) => Ok(done(snap)),
            None => Err(e),
        },
    }
}

/// Anthropic reports no renewal date. When it says whether the plan bills
/// monthly or yearly, the subscription's start day gives an estimate.
fn with_billing(mut local: Local, snap: &Snapshot) -> Local {
    if let Some(hint) = &snap.renewal_hint {
        local.billing = Some(hint.clone());
    }
    local
}

fn window(v: &Value, kind: WindowKind, label: &str, secs: i64) -> Option<Window> {
    let w = v.as_object()?;
    Some(Window {
        kind,
        label: label.to_string(),
        used_pct: w.get("utilization").and_then(Value::as_f64)?,
        resets_at: w.get("resets_at").and_then(Value::as_str).and_then(rfc3339),
        window_secs: Some(secs),
    })
}

/// What a grant clears, in the words the CLI uses for each limit.
fn limit_name(key: &str) -> &str {
    match key {
        "five_hour" => "5h",
        "seven_day" => "week",
        "seven_day_opus" => "Opus",
        "seven_day_sonnet" => "Sonnet",
        "seven_day_overage_included" => "Fable",
        "seven_day_cowork" => "Cowork",
        other => other,
    }
}

pub(crate) fn parse(body: &Value, now: DateTime<Utc>) -> Snapshot {
    const WEEK: i64 = 7 * 86400;
    let mut windows = Vec::new();
    if let Some(w) = body
        .get("five_hour")
        .and_then(|v| window(v, WindowKind::Session, "5h", 18000))
    {
        windows.push(w);
    }
    if let Some(w) = body
        .get("seven_day")
        .and_then(|v| window(v, WindowKind::Weekly, "week", WEEK))
    {
        windows.push(w);
    }
    // Scoped weekly limits: one model or one surface. `limits` also repeats
    // the account-wide row (`weekly_all`), which is skipped.
    if let Some(limits) = body.get("limits").and_then(Value::as_array) {
        for lim in limits {
            if lim.get("kind").and_then(Value::as_str) != Some("weekly_scoped") {
                continue;
            }
            let name = lim
                .pointer("/scope/model/display_name")
                .or_else(|| lim.pointer("/scope/surface/display_name"))
                .and_then(Value::as_str);
            let (Some(name), Some(pct)) = (name, lim.get("percent").and_then(Value::as_f64)) else {
                continue;
            };
            windows.push(Window {
                kind: WindowKind::WeeklyScoped,
                label: name.to_string(),
                used_pct: pct,
                resets_at: lim
                    .get("resets_at")
                    .and_then(Value::as_str)
                    .and_then(rfc3339),
                window_secs: Some(WEEK),
            });
        }
    }
    // Older answers carry per-model windows as their own keys.
    for (key, label) in [("seven_day_opus", "Opus"), ("seven_day_sonnet", "Sonnet")] {
        if windows.iter().any(|w| w.label == label) {
            continue;
        }
        if let Some(w) = body
            .get(key)
            .and_then(|v| window(v, WindowKind::WeeklyScoped, label, WEEK))
        {
            windows.push(w);
        }
    }

    // Extra usage is counted in minor units (cents).
    let credits = body.get("extra_usage").and_then(|e| {
        if e.get("is_enabled").and_then(Value::as_bool) != Some(true) {
            return None;
        }
        let places = e.get("decimal_places").and_then(Value::as_i64).unwrap_or(2);
        let unit = 10f64.powi(places as i32);
        let money = |k: &str| e.get(k).and_then(Value::as_f64).map(|v| v / unit);
        Some(Credits {
            enabled: true,
            used: money("used_credits"),
            limit: money("monthly_limit"),
            balance: None,
            unit: e
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD")
                .to_string(),
            resets_at: None,
        })
    });

    let ember = body.get("cedar_ember").filter(|c| c.is_object());
    let mut grants = Vec::new();
    for g in ember
        .and_then(|c| c.get("grants"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let left = g.get("resets_left").and_then(Value::as_u64).unwrap_or(0) as u32;
        let expires_at = g.get("ends_at").and_then(Value::as_str).and_then(rfc3339);
        if left == 0
            || g.get("paused").and_then(Value::as_bool) == Some(true)
            || expires_at.is_some_and(|t| t <= now)
        {
            continue;
        }
        let clears: Vec<&str> = g
            .get("clears")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(limit_name).collect())
            .unwrap_or_default();
        let scope = if clears.is_empty() {
            g.get("type")
                .and_then(Value::as_str)
                .unwrap_or("limits")
                .to_string()
        } else {
            clears.join(" + ")
        };
        grants.push(Grant {
            scope,
            count: left,
            expires_at,
            note: g
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            source: Source::Api,
            usable_now: g.get("usable_now").and_then(Value::as_bool),
        });
    }
    grants.sort_by_key(|g| g.expires_at.unwrap_or(DateTime::<Utc>::MAX_UTC));

    let mut snap = Snapshot::new(windows);
    snap.fetched_at = now;
    snap.credits = credits;
    snap.grants = grants;
    snap.renewal_hint = ember
        .and_then(|c| c.pointer("/event_props/billing_period"))
        .and_then(Value::as_str)
        .and_then(|p| match p {
            "monthly" => Some(Cycle::Monthly),
            "annual" => Some(Cycle::Yearly),
            _ => None,
        })
        .map(|cycle| Billing {
            cycle: Some(cycle),
            renews_on: None,
            cancelled: false,
            source: Source::Api,
        });
    snap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_is_anthropics_name_else_the_profiles_tier_else_the_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let write = |profile: Value, token: Value| {
            let file = |name: &str, v: Value| {
                std::fs::write(dir.path().join(name), v.to_string()).unwrap();
            };
            file(".claude.json", json!({ "oauthAccount": profile }));
            file(".credentials.json", json!({ "claudeAiOauth": token }));
            local(dir.path(), false).plan
        };
        let old_token = json!({"accessToken": "t", "rateLimitTier": "default_claude_max_5x", "subscriptionType": "max"});

        // After an upgrade the token still names the old tier. The profile
        // has the new one, and it wins.
        let plan = write(
            json!({"organizationRateLimitTier": "default_claude_max_20x"}),
            old_token.clone(),
        );
        assert_eq!(plan.as_deref(), Some("Ultra"));
        // Anthropic's own name beats anything worked out from a tier.
        let plan = write(
            json!({"planDisplayName": "Team Premium", "organizationRateLimitTier": "default_claude_max_20x"}),
            old_token.clone(),
        );
        assert_eq!(plan.as_deref(), Some("Team Premium"));
        // No profile tier: the token's tier, under its current name.
        assert_eq!(write(json!({}), old_token).as_deref(), Some("Max"));
        // No tier at all: the kind of subscription.
        let plan = write(
            json!({}),
            json!({"accessToken": "t", "subscriptionType": "pro"}),
        );
        assert_eq!(plan.as_deref(), Some("Pro"));
        // A tier cswap has no name for is shown as it is.
        assert_eq!(plan_name("default_claude_team_premium"), "Team Premium");
    }

    fn live_shape() -> Value {
        json!({
            "five_hour": null,
            "seven_day": {"utilization": 76.0, "resets_at": "2099-11-11T19:59:59.928929+00:00"},
            "seven_day_opus": null,
            "extra_usage": {"is_enabled": false, "monthly_limit": null},
            "limits": [
                {"kind": "weekly_all", "percent": 76, "resets_at": "2099-11-11T19:59:59+00:00", "scope": null},
                {"kind": "weekly_scoped", "percent": 50, "resets_at": "2099-11-11T19:59:59+00:00",
                 "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}},
                "garbage"
            ],
            "cedar_ember": {
                "eligible": true,
                "grants": [
                    {"id": "full-reset-2611", "label": "One full limit reset (November 2026)",
                     "resets_total": 1, "resets_left": 1, "ends_at": "2099-12-10T22:00:00+00:00",
                     "clears": ["seven_day", "seven_day_overage_included"], "paused": false,
                     "usable_now": true, "type": "weekly"},
                    {"id": "spent", "resets_left": 0, "ends_at": "2099-12-10T22:00:00+00:00"},
                    {"id": "gone", "resets_left": 1, "ends_at": "2020-01-01T00:00:00+00:00"},
                    {"id": "held", "resets_left": 1, "paused": true, "ends_at": "2099-01-01T00:00:00+00:00"}
                ],
                "event_props": {"billing_period": "annual"}
            }
        })
    }

    #[test]
    fn parses_the_weekly_window_and_scoped_limits_without_doubling_the_account_row() {
        let snap = parse(&live_shape(), Utc::now());
        assert!(snap.session().is_none(), "a null five_hour is no window");
        assert_eq!(snap.weekly().unwrap().used_pct, 76.0);
        assert_eq!(snap.windows.len(), 2);
        let fable = &snap.windows[1];
        assert_eq!(
            (fable.kind, fable.label.as_str()),
            (WindowKind::WeeklyScoped, "Fable")
        );
        assert!(snap.credits.is_none());
    }

    #[test]
    fn lists_only_grants_that_can_still_be_spent() {
        let snap = parse(&live_shape(), Utc::now());
        assert_eq!(snap.grants.len(), 1);
        let g = &snap.grants[0];
        assert_eq!(g.scope, "week + Fable");
        assert_eq!(g.count, 1);
        assert_eq!(g.usable_now, Some(true));
        assert!(g.note.contains("November 2026"));
        assert_eq!(snap.renewal_hint.unwrap().cycle, Some(Cycle::Yearly));
    }

    #[test]
    fn extra_usage_is_in_cents() {
        let body = json!({"extra_usage": {"is_enabled": true, "monthly_limit": 5000,
                                           "used_credits": 1234, "currency": "USD"}});
        let c = parse(&body, Utc::now()).credits.unwrap();
        assert_eq!((c.used, c.limit), (Some(12.34), Some(50.0)));
    }

    #[test]
    fn junk_yields_an_empty_snapshot() {
        let snap = parse(&json!({"five_hour": "nope", "limits": 3}), Utc::now());
        assert!(snap.windows.is_empty() && snap.grants.is_empty());
    }

    #[test]
    fn the_disk_cache_is_ignored_when_it_belongs_to_another_account() {
        let dir = tempfile::tempdir().unwrap();
        let write = |owner: &str| {
            std::fs::write(
                dir.path().join(".claude.json"),
                json!({
                    "oauthAccount": {"accountUuid": "me"},
                    "cachedUsageUtilization": {"fetchedAtMs": 1794000000000i64,
                        "accountUuid": owner, "utilization": {"seven_day": {"utilization": 5.0}}}
                })
                .to_string(),
            )
            .unwrap();
        };
        write("me");
        assert!(disk_cache(dir.path(), false).is_some());
        write("someone-else");
        assert!(disk_cache(dir.path(), false).is_none());
    }

    #[test]
    fn a_new_account_is_seeded_without_any_identity() {
        let sb = crate::testutil::Sandbox::new();
        sb.native(Harness::Claude);
        std::fs::write(
            paths::home().join(".claude.json"),
            json!({"oauthAccount": {"emailAddress": "live@x.com"}, "theme": "light",
                   "mcpServers": {"srv": {}}, "projects": {"/r": {}}})
            .to_string(),
        )
        .unwrap();
        let dir = sb.root().join("acc");
        std::fs::create_dir_all(&dir).unwrap();
        seed(&dir).unwrap();
        let seeded = fsx::read_value(&dir.join(".claude.json")).unwrap();
        assert_eq!(seeded["theme"], "light");
        assert_eq!(seeded["hasCompletedOnboarding"], true);
        assert!(seeded.get("mcpServers").is_some());
        assert!(seeded.get("oauthAccount").is_none());
    }
}
