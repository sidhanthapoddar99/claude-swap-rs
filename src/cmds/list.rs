//! `cswap list` — Default/Active as standalone entity lines (email only),
//! then every profile with its aliases and usage split across one line per
//! window (5h / 7d / per-model weekly).
//!
//! Colors mirror the user's statusline convention: <70 green, <90 yellow,
//! else red; labels and reset times dimmed. Disabled when stdout is not a
//! terminal or NO_COLOR is set.

use anyhow::Result;
use std::io::IsTerminal;

use crate::config::{Account, Config};
use crate::{oauth, profile};

const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

fn color_on() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn pct_color(pct: f64) -> &'static str {
    if pct < 70.0 {
        GREEN
    } else if pct < 90.0 {
        YELLOW
    } else {
        RED
    }
}

pub fn run(quick: bool) -> Result<()> {
    print_table(quick)?;
    crate::update_check::nudge();
    Ok(())
}

pub fn print_table(quick: bool) -> Result<()> {
    let cfg = Config::load()?;
    if cfg.accounts.is_empty() {
        println!("No accounts yet. Run: cswap login");
        return Ok(());
    }
    let color = color_on();
    let dim = |s: &str| {
        if color {
            format!("{DIM}{s}{RESET}")
        } else {
            s.to_string()
        }
    };

    // Default and Active are entities of their own: email only, no aliases.
    match &cfg.default {
        Some(d) => println!("Default: {d}"),
        None => println!(
            "Default: {}",
            dim("(not set — cswap default <alias|email>)")
        ),
    }
    let active_email = std::env::var("CSWAP_ACTIVE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|k| cfg.find(&k).map(|a| a.email.clone()).unwrap_or(k));
    if let Some(email) = &active_email {
        println!("Active:  {email} {}", dim("[this shell]"));
    }
    println!();

    for acct in &cfg.accounts {
        let mut marker = String::new();
        if active_email.as_deref() == Some(acct.email.as_str()) {
            marker.push('*');
        }
        if cfg.default.as_deref() == Some(acct.email.as_str()) {
            marker.push('d');
        }
        let aliases = if acct.aliases.is_empty() {
            dim("(no alias)")
        } else {
            acct.aliases.join(", ")
        };
        let iso = if acct.isolated {
            dim(" [isolated]")
        } else {
            String::new()
        };
        println!("{marker:<2} {aliases:<16} {}{iso}", acct.email);
        if !quick {
            for line in usage_lines(acct, color) {
                println!("     {line}");
            }
        }
    }
    println!(
        "{}",
        dim(if quick {
            "   (* active in this shell, d default) — usage skipped (--quick)"
        } else {
            "   (* active in this shell, d default)"
        })
    );
    Ok(())
}

/// One line per usage window, e.g. ["5h     3%  in 4h39m (02:10)", ...].
fn usage_lines(acct: &Account, color: bool) -> Vec<String> {
    match try_usage(acct, color) {
        Ok(lines) if lines.is_empty() => vec!["no window data".to_string()],
        Ok(lines) => lines,
        Err(e) => vec![format!("usage unavailable ({e:#})")],
    }
}

fn try_usage(acct: &Account, color: bool) -> Result<Vec<String>> {
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
    Ok(oauth::windows(&usage)
        .iter()
        .map(|w| {
            let reset = w
                .resets_at
                .as_deref()
                .and_then(oauth::format_reset)
                .unwrap_or_default();
            if color {
                format!(
                    "{DIM}{:<6}{RESET}{}{:>4.0}%{RESET}  {DIM}{}{RESET}",
                    w.label,
                    pct_color(w.pct),
                    w.pct,
                    reset
                )
            } else {
                format!("{:<6}{:>4.0}%  {}", w.label, w.pct, reset)
            }
        })
        .collect())
}
