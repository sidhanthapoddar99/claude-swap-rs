//! `cswap list` — accounts, default/active markers, and usage windows.

use anyhow::Result;

use crate::config::{Account, Config};
use crate::{oauth, profile};

pub fn run(quick: bool) -> Result<()> {
    print_table(quick)
}

pub fn print_table(quick: bool) -> Result<()> {
    let cfg = Config::load()?;
    if cfg.accounts.is_empty() {
        println!("No accounts yet. Log into Claude Code, then run: cswap login");
        return Ok(());
    }
    let active = std::env::var("CSWAP_ACTIVE").ok().filter(|s| !s.is_empty());

    println!("{:<2} {:<14} {:<30} USAGE", "", "NAME", "EMAIL");
    for acct in &cfg.accounts {
        let mut marker = String::new();
        if active.as_deref() == Some(acct.name.as_str()) {
            marker.push('*');
        }
        if cfg.default.as_deref() == Some(acct.name.as_str()) {
            marker.push('d');
        }
        let usage = if quick {
            "-".to_string()
        } else {
            usage_line(acct)
        };
        let iso = if acct.isolated { " [isolated]" } else { "" };
        println!(
            "{marker:<2} {:<14} {:<30} {usage}{iso}",
            acct.name, acct.email
        );
    }
    let mut legend = String::from("   (* active in this shell, d default)");
    if quick {
        legend.push_str("  — usage skipped (--quick)");
    }
    println!("{legend}");
    Ok(())
}

fn usage_line(acct: &Account) -> String {
    match try_usage(acct) {
        Ok(line) if line.is_empty() => "no window data".to_string(),
        Ok(line) => line,
        Err(e) => format!("usage unavailable ({e:#})"),
    }
}

fn try_usage(acct: &Account) -> Result<String> {
    // Live account: read ~/.claude's token as-is, never refresh it (rotation
    // is claude's job for the live login). Others: profile creds + refresh.
    let creds = if profile::live_email().as_deref() == Some(acct.email.as_str()) {
        let text = std::fs::read_to_string(crate::paths::live_credentials())?;
        let creds: serde_json::Value = serde_json::from_str(&text)?;
        let fresh = creds
            .get("claudeAiOauth")
            .and_then(|o| o.get("expiresAt"))
            .and_then(serde_json::Value::as_i64)
            .map(|t| t > oauth::now_ms())
            .unwrap_or(false);
        if !fresh {
            anyhow::bail!("live token expired — run claude once to refresh");
        }
        creds
    } else {
        profile::current_creds(acct)?
    };
    let token = oauth::access_token(&creds).ok_or_else(|| anyhow::anyhow!("no access token"))?;
    let usage = oauth::fetch_usage(token)?;
    let parts: Vec<String> = oauth::windows(&usage)
        .iter()
        .map(|w| {
            let reset = w
                .resets_at
                .as_deref()
                .and_then(oauth::format_reset)
                .map(|r| format!(" {r}"))
                .unwrap_or_default();
            format!("{} {:.0}%{}", w.label, w.pct, reset)
        })
        .collect();
    Ok(parts.join(" | "))
}
