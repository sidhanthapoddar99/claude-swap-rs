//! `cswap list` — default/active shown as their own entities, then all
//! profiles (keyed by email) with aliases and colored usage windows.
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
        println!("No accounts yet. Log into Claude Code, then run: cswap login");
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

    // The default and this shell's active account are entities of their own.
    match cfg.default.as_deref().and_then(|d| cfg.find(d)) {
        Some(a) => println!("Default: {} {}", a.name, dim(&format!("({})", a.email))),
        None => println!("Default: {}", dim("(not set — cswap default <name>)")),
    }
    let active = std::env::var("CSWAP_ACTIVE").ok().filter(|s| !s.is_empty());
    if let Some(active_name) = &active {
        match cfg.find(active_name) {
            Some(a) => println!(
                "Active:  {} {} {}",
                a.name,
                dim(&format!("({})", a.email)),
                dim("[this shell]")
            ),
            None => println!("Active:  {active_name} {}", dim("[unknown account!]")),
        }
    }
    println!();

    println!(
        "{}",
        dim(&format!(
            "{:<2} {:<14} {:<12} {:<30} USAGE",
            "", "NAME", "ALIASES", "EMAIL"
        ))
    );
    for acct in &cfg.accounts {
        let mut marker = String::new();
        if active.as_deref() == Some(acct.name.as_str())
            || active
                .as_deref()
                .map(|k| acct.aliases.iter().any(|al| al == k))
                .unwrap_or(false)
        {
            marker.push('*');
        }
        if cfg.default.as_deref() == Some(acct.name.as_str()) {
            marker.push('d');
        }
        let aliases = if acct.aliases.is_empty() {
            "-".to_string()
        } else {
            acct.aliases.join(",")
        };
        let usage = if quick {
            dim("-")
        } else {
            usage_line(acct, color)
        };
        let iso = if acct.isolated {
            dim(" [isolated]")
        } else {
            String::new()
        };
        println!(
            "{marker:<2} {:<14} {aliases:<12} {:<30} {usage}{iso}",
            acct.name, acct.email
        );
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

fn usage_line(acct: &Account, color: bool) -> String {
    match try_usage(acct, color) {
        Ok(line) if line.is_empty() => "no window data".to_string(),
        Ok(line) => line,
        Err(e) => format!("usage unavailable ({e:#})"),
    }
}

fn try_usage(acct: &Account, color: bool) -> Result<String> {
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
            if color {
                format!(
                    "{DIM}{}{RESET} {}{:.0}%{RESET}{DIM}{}{RESET}",
                    w.label,
                    pct_color(w.pct),
                    w.pct,
                    reset
                )
            } else {
                format!("{} {:.0}%{}", w.label, w.pct, reset)
            }
        })
        .collect();
    Ok(parts.join(if color { " \x1b[2m|\x1b[0m " } else { " | " }))
}
