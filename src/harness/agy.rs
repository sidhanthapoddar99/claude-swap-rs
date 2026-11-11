//! Google Antigravity CLI (`agy`). Experimental.
//!
//! Checked against agy 1.5.5, with two parts that could not be exercised
//! without a live login and are marked below.
//!
//! - There is no env var that moves agy's state. The undocumented flag
//!   `--gemini_dir=<absolute path>` moves the whole `~/.gemini` tree, so a
//!   cswap account is launched with that flag in front of the user's arguments.
//! - agy keeps its login in one fixed OS keyring entry that does not depend on
//!   that flag, so two accounts would overwrite each other. A cswap account is
//!   therefore launched with no session bus address, which makes agy fall back
//!   to a token file inside the account's own dir. The cost: programs that need
//!   the session bus (desktop notifications, for one) fail inside that agent's
//!   shells. Loading from the file is verified; saving a fresh login to it is
//!   the documented fallback but UNVERIFIED here.
//! - Usage is asked of agy itself (`agy -p "/usage" --output-format json`),
//!   which the vendor says spends no quota. So cswap never reads or renews an
//!   Antigravity token. The shape of that JSON is UNVERIFIED: the reader below
//!   looks for quota buckets wherever they are, in either naming style.
//!
//! A fresh config dir makes agy run its self-updater, which replaces the shared
//! binary, so every cswap launch sets `AGY_CLI_DISABLE_AUTO_UPDATE=true`
//! (`1` does not work).

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use super::{rfc3339, unix, Fetched, Harness, Local, Spec};
use crate::http::{Fetch, FetchError, Why};
use crate::model::{Credits, Snapshot, Window, WindowKind};
use crate::store::{HarnessSettings, Identity};
use crate::{fsx, paths};

const TOKEN_FILE: &str = "antigravity-cli/antigravity-oauth-token";

pub const SPEC: Spec = Spec {
    id: "agy",
    name: "Antigravity",
    vendor: "Google",
    bins: &["agy"],
    home_env: None,
    home_flag: Some("--gemini_dir="),
    nested: &["antigravity-cli", "antigravity-cli/cache"],
    experimental: true,
    native: ".gemini",
    private: &[
        "antigravity-cli/antigravity-oauth-token",
        // Per-install and per-account markers.
        "antigravity-cli/jetski_state.pbtxt",
        "antigravity-cli/installation_id",
        "antigravity-cli/cache/onboarding.json",
        "antigravity-cli/cache/default_project_id.txt",
        // Logs, crash dumps, scratch space and the updater's own files.
        "antigravity-cli/log",
        "antigravity-cli/cli.log",
        "antigravity-cli/crashes",
        "antigravity-cli/scratch",
        "antigravity-cli/updater",
        "antigravity-cli/last_check.timestamp",
    ],
    cred_file: TOKEN_FILE,
    login_life: "Access token lasts 1 hour and agy renews it by itself. Google publishes no fixed lifetime for the login; it ends on sign-out or when access is revoked in your Google account.",
    scrub_env: &["GEMINI_API_KEY", "AGY_ADC_AUTH"],
    // agy has no sign-in subcommand: started on an empty dir it walks its own
    // sign-in, and the user quits when done.
    login_args: &[],
    rgb: (76, 141, 246),
};

/// What a launch on a cswap account needs in its environment.
pub fn account_env() -> Vec<(&'static str, OsString)> {
    vec![
        ("AGY_CLI_DISABLE_AUTO_UPDATE", "true".into()),
        // No reachable session bus: agy stores the login in the account's own
        // token file instead of the one shared keyring entry.
        (
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/cswap-no-session-bus".into(),
        ),
    ]
}

fn token(config_dir: &Path) -> Option<Value> {
    fsx::read_value(&config_dir.join(TOKEN_FILE))
}

/// A cswap account is signed in when its token file holds a token. The native
/// login lives in the OS keyring, which cswap does not read: it counts as
/// present when agy has been set up, and a fetch says otherwise if it is not.
pub fn logged_in(config_dir: &Path, native: bool) -> bool {
    if native {
        return config_dir.join("antigravity-cli").is_dir();
    }
    token(config_dir).is_some_and(|t| {
        t.pointer("/token/refresh_token")
            .or_else(|| t.pointer("/token/access_token"))
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    })
}

pub fn local(config_dir: &Path, _native: bool) -> Local {
    let Some(t) = token(config_dir) else {
        return Local::default();
    };
    Local {
        identity: Identity {
            email: t
                .get("id_token_email")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from),
            ..Identity::default()
        },
        plan: None,
        token_expires: t
            .pointer("/token/expiry")
            .and_then(Value::as_str)
            .and_then(rfc3339),
        login_expires: None,
        billing: None,
    }
}

/// Run a command to completion or kill it after `secs`. Returns the exit
/// success flag, stdout and stderr.
fn run_bounded(mut cmd: Command, secs: u64) -> Fetch<(bool, String, String)> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| FetchError::new(Why::Unsupported, format!("cannot run agy ({e})")))?;
    // Drain both pipes on threads, so a chatty child cannot block on a full
    // pipe while this thread waits for it to exit.
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed().as_secs() >= secs => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(FetchError::new(
                    Why::Network,
                    format!("agy did not answer within {secs}s"),
                ));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(60)),
            Err(e) => return Err(FetchError::new(Why::Shape, format!("agy failed ({e})"))),
        }
    };
    Ok((
        status.success(),
        out_t.join().unwrap_or_default(),
        err_t.join().unwrap_or_default(),
    ))
}

pub fn fetch(config_dir: &Path, native: bool) -> Fetch<Fetched> {
    if !logged_in(config_dir, native) {
        return Err(FetchError::new(Why::NoLogin, "not signed in"));
    }
    let configured = HarnessSettings::load(Harness::Agy)
        .ok()
        .and_then(|h| h.binary);
    let bin = Harness::Agy
        .binary(configured.as_deref())
        .map_err(|e| FetchError::new(Why::Unsupported, format!("{e:#}")))?;
    // A neutral working dir, so asking for usage never registers a project.
    let cwd = paths::cache_dir().join("agy-usage");
    let _ = fsx::private_dir(&cwd);

    let mut cmd = Command::new(bin);
    if !native {
        cmd.args(Harness::Agy.launch_args(config_dir));
        for (k, v) in account_env() {
            cmd.env(k, v);
        }
    }
    cmd.env("AGY_CLI_DISABLE_AUTO_UPDATE", "true");
    cmd.args(["-p", "/usage", "--output-format", "json"]);
    cmd.current_dir(&cwd);

    let (ok, stdout, stderr) = run_bounded(cmd, 30)?;
    let mut local = local(config_dir, native);
    match parse(&stdout, Utc::now()) {
        Ok((snapshot, seen)) => {
            if seen.identity.email.is_some() {
                local.identity.email = seen.identity.email;
            }
            if seen.plan.is_some() {
                local.plan = seen.plan;
            }
            Ok(Fetched { snapshot, local })
        }
        Err(e) => {
            let said = format!("{stdout}\n{stderr}").to_lowercase();
            if [
                "sign in",
                "signed in",
                "log in",
                "login",
                "unauthenticated",
                "not authenticated",
            ]
            .iter()
            .any(|w| said.contains(w))
            {
                return Err(FetchError::new(
                    Why::Rejected,
                    "agy says it is not signed in: run agy once on this account",
                ));
            }
            if !ok {
                let line = stderr
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("no message");
                let short: String = line.trim().chars().take(100).collect();
                return Err(FetchError::new(
                    Why::Shape,
                    format!("agy /usage failed ({short})"),
                ));
            }
            Err(e)
        }
    }
}

fn get<'a>(v: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|n| v.get(n))
}

fn text(v: &Value, names: &[&str]) -> Option<String> {
    get(v, names)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
}

/// A timestamp as RFC 3339 text, as unix seconds, or as protobuf
/// `{seconds, nanos}`.
fn time(v: &Value) -> Option<DateTime<Utc>> {
    match v {
        Value::String(s) => rfc3339(s).or_else(|| s.parse::<i64>().ok().and_then(unix)),
        Value::Number(n) => n.as_i64().and_then(unix),
        Value::Object(_) => v
            .get("seconds")
            .and_then(|s| s.as_i64().or_else(|| s.as_str()?.parse().ok()))
            .and_then(unix),
        _ => None,
    }
}

struct Bucket {
    label: String,
    window: String,
    used_pct: f64,
    resets_at: Option<DateTime<Utc>>,
}

/// A group's name as a short label: `Gemini Models` is `Gemini`, and
/// `Claude and GPT models` is `Claude & GPT`.
fn tidy(name: &str) -> String {
    let name = name.trim();
    let cut = name.len().saturating_sub(7);
    let name = match name.get(cut..) {
        Some(tail) if tail.eq_ignore_ascii_case(" models") => &name[..cut],
        _ => name,
    };
    name.replace(" and ", " & ")
}

/// A quota bucket, if `v` is one. A bucket inside a group is named after the
/// group (`owner`): its own name only says which window it is, such as
/// "Weekly Limit Remaining".
fn bucket(v: &Value, owner: Option<&str>, fallback: Option<&str>) -> Option<Bucket> {
    let quota = get(v, &["quotaInfo", "quota_info"]).unwrap_or(v);
    let fraction = get(quota, &["remainingFraction", "remaining_fraction"])
        .and_then(|f| f.as_f64().or_else(|| f.as_str()?.parse().ok()))?;
    if get(v, &["disabled"]).and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let label = owner
        .map(String::from)
        .or_else(|| text(v, &["displayName", "display_name"]))
        .or_else(|| text(v, &["modelId", "model_id", "bucketId", "bucket_id", "name"]))
        .or_else(|| fallback.map(String::from))
        .unwrap_or_else(|| "quota".to_string());
    Some(Bucket {
        label,
        window: text(v, &["window", "refreshWindowLabel", "refresh_window_label"])
            .unwrap_or_default(),
        used_pct: ((1.0 - fraction) * 100.0).clamp(0.0, 100.0),
        resets_at: get(quota, &["resetTime", "reset_time"]).and_then(time),
    })
}

/// Collect every object that looks like a quota bucket, wherever it sits.
fn walk(v: &Value, group: Option<&str>, out: &mut Vec<Bucket>) {
    match v {
        Value::Array(items) => items.iter().for_each(|i| walk(i, group, out)),
        Value::Object(map) => {
            // What agy 1.5 prints: groups of models, each with its buckets.
            if let Some(Value::Array(items)) = get(v, &["buckets"]) {
                let owner = text(v, &["displayName", "display_name", "name"]).map(|n| tidy(&n));
                for item in items {
                    match bucket(item, owner.as_deref(), group) {
                        Some(b) => out.push(b),
                        None => walk(item, owner.as_deref().or(group), out),
                    }
                }
                return;
            }
            if let Some(b) = bucket(v, None, group) {
                out.push(b);
                return;
            }
            let name = text(v, &["displayName", "display_name"]);
            let group = name.as_deref().or(group);
            for (key, child) in map {
                // A map of models keyed by id: the key is the best name.
                let g = if matches!(key.as_str(), "models") {
                    None
                } else {
                    group
                };
                match child {
                    Value::Object(inner) if key == "models" => {
                        for (id, m) in inner {
                            walk(m, Some(id), out);
                        }
                    }
                    _ => walk(child, g, out),
                }
            }
        }
        _ => {}
    }
}

/// Work out what a bucket measures from the vendor's window text, or failing
/// that from how far away its reset is.
fn classify(b: &Bucket, now: DateTime<Utc>) -> (WindowKind, Option<i64>) {
    let w = b.window.to_lowercase();
    let by_text = if w.contains("week") || w.contains("7d") || w.contains("7 d") {
        Some(WindowKind::Weekly)
    } else if w.contains("month") {
        Some(WindowKind::Monthly)
    } else if w.contains("hour") || w.ends_with('h') {
        Some(WindowKind::Session)
    } else if w.contains("day") || w.contains("daily") {
        Some(WindowKind::Daily)
    } else {
        None
    };
    let kind = by_text.unwrap_or_else(|| match b.resets_at {
        Some(t) if t - now <= Duration::hours(6) => WindowKind::Session,
        Some(t) if t - now <= Duration::hours(36) => WindowKind::Daily,
        Some(t) if t - now <= Duration::days(8) => WindowKind::Weekly,
        Some(_) => WindowKind::Monthly,
        None => WindowKind::Other,
    });
    let secs = match kind {
        WindowKind::Session => Some(5 * 3600),
        WindowKind::Daily => Some(86400),
        WindowKind::Weekly => Some(7 * 86400),
        _ => None,
    };
    (kind, secs)
}

/// The first string found under any of `names`, anywhere in the document.
fn find_text(v: &Value, names: &[&str]) -> Option<String> {
    match v {
        Value::Object(map) => {
            text(v, names).or_else(|| map.values().find_map(|c| find_text(c, names)))
        }
        Value::Array(items) => items.iter().find_map(|c| find_text(c, names)),
        _ => None,
    }
}

fn find_number(v: &Value, names: &[&str]) -> Option<f64> {
    match v {
        Value::Object(map) => get(v, names)
            .and_then(|n| n.as_f64().or_else(|| n.as_str()?.parse().ok()))
            .or_else(|| map.values().find_map(|c| find_number(c, names))),
        Value::Array(items) => items.iter().find_map(|c| find_number(c, names)),
        _ => None,
    }
}

/// Read agy's `/usage` answer: one JSON document, or one per line.
pub(crate) fn parse(stdout: &str, now: DateTime<Utc>) -> Fetch<(Snapshot, Local)> {
    let docs: Vec<Value> = match serde_json::from_str::<Value>(stdout.trim()) {
        Ok(v) => vec![v],
        Err(_) => stdout
            .lines()
            .filter_map(|l| serde_json::from_str(l.trim()).ok())
            .collect(),
    };
    if docs.is_empty() {
        return Err(FetchError::new(
            Why::Shape,
            "agy /usage did not answer in JSON; this agy version may not support it",
        ));
    }
    let doc = Value::Array(docs);
    let mut buckets = Vec::new();
    walk(&doc, None, &mut buckets);
    if buckets.is_empty() {
        return Err(FetchError::new(
            Why::Shape,
            "agy /usage answered, but with no quota cswap can read",
        ));
    }

    let mut windows: Vec<Window> = buckets
        .iter()
        .map(|b| {
            let (kind, window_secs) = classify(b, now);
            Window {
                kind,
                label: b.label.clone(),
                used_pct: b.used_pct,
                resets_at: b.resets_at,
                window_secs,
            }
        })
        .collect();
    // Antigravity meters each group of models on its own. The fullest weekly
    // bucket stands for the account and the others are shown as scoped
    // limits, each under its group's name. Of the short windows only the
    // fullest is kept: it is the one that can stop you.
    for (kind, short) in [(WindowKind::Weekly, "week"), (WindowKind::Session, "5h")] {
        let of_kind: Vec<usize> = (0..windows.len())
            .filter(|i| windows[*i].kind == kind)
            .collect();
        let Some(lead) = of_kind
            .iter()
            .copied()
            .max_by(|a, b| windows[*a].used_pct.total_cmp(&windows[*b].used_pct))
        else {
            continue;
        };
        if of_kind.len() == 1 || kind == WindowKind::Session {
            windows[lead].label = short.to_string();
        }
        for i in of_kind.into_iter().rev().filter(|i| *i != lead) {
            if kind == WindowKind::Session {
                windows.remove(i);
            } else {
                windows[i].kind = WindowKind::WeeklyScoped;
            }
        }
    }
    windows.sort_by_key(|w| match w.kind {
        WindowKind::Session => 0,
        WindowKind::Daily => 1,
        WindowKind::Weekly => 2,
        WindowKind::Monthly => 3,
        WindowKind::WeeklyScoped => 4,
        WindowKind::Other => 5,
    });

    let mut snapshot = Snapshot::new(windows);
    snapshot.fetched_at = now;
    if let Some(balance) = find_number(
        &doc,
        &[
            "creditAmount",
            "credit_amount",
            "remainingCredits",
            "remaining_credits",
        ],
    ) {
        if balance > 0.0 {
            snapshot.credits = Some(Credits {
                enabled: true,
                balance: Some(balance),
                unit: "AI credits".to_string(),
                ..Credits::default()
            });
        }
    }
    let local = Local {
        identity: Identity {
            email: find_text(
                &doc,
                &["email", "id_token_email", "accountEmail", "account_email"],
            ),
            ..Identity::default()
        },
        plan: find_text(
            &doc,
            &[
                "tierDisplayName",
                "tier_display_name",
                "tierName",
                "tier_name",
                "plan",
            ],
        ),
        ..Local::default()
    };
    Ok((snapshot, local))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_summary_buckets_in_either_naming_style() {
        let now = Utc::now();
        let week = (now + Duration::days(4)).to_rfc3339();
        let soon = (now + Duration::hours(3)).to_rfc3339();
        let doc = json!({
            "tier_display_name": "Google AI Pro",
            "groups": [{
                "displayName": "Gemini Pro",
                "buckets": [
                    {"bucketId": "a", "window": "5 hours", "remainingFraction": 0.75, "resetTime": soon},
                    {"bucketId": "b", "window": "weekly", "remaining_fraction": 0.4, "reset_time": week}
                ]
            }]
        });
        let (snap, local) = parse(&doc.to_string(), now).unwrap();
        assert_eq!(local.plan.as_deref(), Some("Google AI Pro"));
        assert_eq!(snap.session().unwrap().used_pct, 25.0);
        let w = snap.weekly().unwrap();
        assert_eq!((w.label.as_str(), w.used_pct.round()), ("week", 60.0));
        assert_eq!(w.window_secs, Some(604800));
    }

    #[test]
    fn several_weekly_buckets_keep_the_fullest_as_the_account_window() {
        let now = Utc::now();
        let week = (now + Duration::days(5)).to_rfc3339();
        let doc = json!({"buckets": [
            {"displayName": "Flash", "window": "weekly", "remainingFraction": 0.9, "resetTime": week},
            {"displayName": "Pro", "window": "weekly", "remainingFraction": 0.2, "resetTime": week}
        ]});
        let (snap, _) = parse(&doc.to_string(), now).unwrap();
        let lead = snap.weekly().unwrap();
        assert_eq!(lead.label, "Pro");
        assert_eq!(lead.used_pct.round(), 80.0);
        assert!(snap
            .windows
            .iter()
            .any(|w| w.kind == WindowKind::WeeklyScoped && w.label == "Flash"));
    }

    #[test]
    fn groups_of_models_as_agy_prints_them() {
        let now = Utc::now();
        let t = |hours: i64| (now + Duration::hours(hours)).to_rfc3339();
        // The shape of `agy -p /usage --output-format json`, agy 1.5.
        let doc = json!({"status": "SUCCESS", "command": {"name": "usage", "data": {"groups": [
            {"name": "Gemini Models", "buckets": [
                {"id": "gemini-weekly", "name": "Weekly Limit Remaining", "window": "weekly",
                 "remaining_fraction": 0.98, "reset_time": t(80)},
                {"id": "gemini-5h", "name": "Five Hour Limit Remaining", "window": "5h",
                 "remaining_fraction": 1, "reset_time": t(4)}
            ]},
            {"name": "Claude and GPT models", "buckets": [
                {"id": "3p-weekly", "name": "Weekly Limit Remaining", "window": "weekly",
                 "remaining_fraction": 1, "reset_time": t(160)},
                {"id": "3p-5h", "name": "Five Hour Limit Remaining", "window": "5h",
                 "remaining_fraction": 0.5, "reset_time": t(4)}
            ]}
        ]}}});
        let (snap, _) = parse(&doc.to_string(), now).unwrap();
        let seen: Vec<(WindowKind, &str, f64)> = snap
            .windows
            .iter()
            .map(|w| (w.kind, w.label.as_str(), w.used_pct.round()))
            .collect();
        assert_eq!(
            seen,
            [
                (WindowKind::Session, "5h", 50.0),
                (WindowKind::Weekly, "Gemini", 2.0),
                (WindowKind::WeeklyScoped, "Claude & GPT", 0.0),
            ]
        );
        assert_eq!(tidy("Gemini Models"), "Gemini");
        assert_eq!(tidy("models"), "models");
    }

    #[test]
    fn models_keyed_by_id_and_windows_inferred_from_the_reset_time() {
        let now = Utc::now();
        let doc = json!({"models": {
            "gemini-3-pro": {"quotaInfo": {"remainingFraction": 0.5,
                              "resetTime": {"seconds": (now + Duration::days(6)).timestamp()}}}
        }});
        let (snap, _) = parse(&doc.to_string(), now).unwrap();
        let w = snap.weekly().unwrap();
        assert_eq!(w.used_pct, 50.0);
        assert!(w.resets_at.is_some());
    }

    #[test]
    fn output_without_quota_is_a_shape_error_not_a_guess() {
        let now = Utc::now();
        assert_eq!(parse("plain text", now).unwrap_err().why, Why::Shape);
        assert_eq!(parse(r#"{"hello": 1}"#, now).unwrap_err().why, Why::Shape);
    }

    #[test]
    fn a_cswap_account_needs_a_token_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!logged_in(dir.path(), false));
        std::fs::create_dir_all(dir.path().join("antigravity-cli")).unwrap();
        std::fs::write(
            dir.path().join(TOKEN_FILE),
            r#"{"token":{"refresh_token":"r","expiry":"2026-11-08T06:34:51+05:30"},"id_token_email":"a@b.c"}"#,
        )
        .unwrap();
        assert!(logged_in(dir.path(), false));
        let l = local(dir.path(), false);
        assert_eq!(l.identity.email.as_deref(), Some("a@b.c"));
        assert!(l.token_expires.is_some());
    }
}
