//! Passive update nudge: at most one GitHub Releases API call per 24h,
//! cached, silent on any failure, and only when stdout is a terminal (so
//! scripts, pipes, and the test suite never trigger network traffic).
//! `CSWAP_NO_UPDATE_CHECK=1` disables it entirely.

use serde_json::{json, Value};
use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;

use crate::oauth::now_ms;
use crate::paths;

pub const REPO: &str = "sidhanthapoddar99/claude-swap-rs";
const CACHE_TTL_MS: i64 = 24 * 3600 * 1000;

fn cache_path() -> PathBuf {
    paths::data_dir().join("update_check.json")
}

/// Print a one-line hint on stderr when a newer release exists. Never errors,
/// never blocks longer than the 2s request timeout, at most once per day.
pub fn nudge() {
    if std::env::var_os("CSWAP_NO_UPDATE_CHECK").is_some() || !std::io::stdout().is_terminal() {
        return;
    }
    let current = env!("CARGO_PKG_VERSION");
    let latest = match cached_or_fetch() {
        Some(v) if !v.is_empty() => v,
        _ => return,
    };
    if version_newer(&latest, current) {
        eprintln!("cswap: v{latest} is available (you have v{current}) — run `cswap upgrade`");
    }
}

fn cached_or_fetch() -> Option<String> {
    let path = cache_path();
    if let Some(cached) = fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        let fresh = cached
            .get("checked_at_ms")
            .and_then(Value::as_i64)
            .map(|t| now_ms() - t < CACHE_TTL_MS)
            .unwrap_or(false);
        if fresh {
            return cached
                .get("latest")
                .and_then(Value::as_str)
                .map(String::from);
        }
    }
    // Cache miss/stale: fetch, then cache even a failure (empty string) so an
    // offline machine doesn't retry on every single command.
    let latest = fetch_latest_version().unwrap_or_default();
    let entry = json!({"checked_at_ms": now_ms(), "latest": latest});
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, entry.to_string());
    Some(latest)
}

/// Latest release version (without the leading 'v'), from the GitHub API.
pub fn fetch_latest_version() -> Option<String> {
    let resp: Value = ureq::get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))
    .set("User-Agent", crate::oauth::USER_AGENT)
    .timeout(std::time::Duration::from_secs(2))
    .call()
    .ok()?
    .into_json()
    .ok()?;
    resp.get("tag_name")
        .and_then(Value::as_str)
        .map(|t| t.trim_start_matches('v').to_string())
}

/// True when `a` is a strictly newer x.y.z than `b`. Non-numeric segments
/// compare as 0, so weird tags can never panic — worst case, no nudge.
pub fn version_newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split('.')
            .map(|s| {
                s.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let (a, b) = (parse(a), parse(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(version_newer("0.2.0", "0.1.0"));
        assert!(version_newer("0.10.0", "0.9.9"));
        assert!(version_newer("1.0.0", "0.99.99"));
        assert!(version_newer("0.1.1", "0.1.0"));
        assert!(!version_newer("0.1.0", "0.1.0"));
        assert!(!version_newer("0.1.0", "0.2.0"));
        assert!(!version_newer("garbage", "0.1.0"));
        assert!(version_newer("0.2.0-beta", "0.1.9")); // suffix ignored
    }
}
