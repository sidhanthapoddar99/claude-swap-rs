//! Shared terminal rendering: color policy, severity ramp, usage bars, and
//! the one place that fetches a window list for an account.
//!
//! The bar glyphs follow the reference project's dashboard — `━` fill, `╸`
//! for the half cell, `─` for the untouched track — so a cswap bar reads the
//! same whichever implementation drew it. Severity mirrors the user's
//! statusline: <70 green, <90 yellow, else red.

use anyhow::Result;
use std::io::IsTerminal;

use crate::config::Account;
use crate::oauth::{self, Window};
use crate::profile;

pub const DIM: &str = "\x1b[2m";
pub const BOLD: &str = "\x1b[1m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const RED: &str = "\x1b[31m";
/// Warm terracotta — the accent the reference dashboard uses for "active".
pub const ACCENT: &str = "\x1b[38;5;173m";
pub const RESET: &str = "\x1b[0m";

const BAR_FILLED: char = '━';
const BAR_HALF: char = '╸';
const BAR_EMPTY: char = '─';

pub fn color_on() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::env::var_os("FORCE_COLOR").is_some() || std::io::stdout().is_terminal()
}

pub fn pct_color(pct: f64) -> &'static str {
    if pct < 70.0 {
        GREEN
    } else if pct < 90.0 {
        YELLOW
    } else {
        RED
    }
}

/// Paint `text` with `codes` when color is on; otherwise return it bare.
pub fn paint(color: bool, codes: &str, text: &str) -> String {
    if color && !text.is_empty() {
        format!("{codes}{text}{RESET}")
    } else {
        text.to_string()
    }
}

/// Left-pad to `width` counting only visible characters — `styled` carries
/// ANSI codes that `{:<w}` would happily (and wrongly) count.
pub fn pad(plain: &str, styled: &str, width: usize) -> String {
    let visible = plain.chars().count();
    format!("{styled}{}", " ".repeat(width.saturating_sub(visible)))
}

/// `━━━━━╸──────────` — `width` cells, filled proportionally to `pct`.
pub fn bar(pct: f64, width: usize, color: bool) -> String {
    let cells = pct.clamp(0.0, 100.0) / 100.0 * width as f64;
    let full = cells.trunc() as usize;
    let half = (cells - cells.trunc()) >= 0.5 && full < width;
    let filled: String = std::iter::repeat_n(BAR_FILLED, full.min(width))
        .chain(if half { Some(BAR_HALF) } else { None })
        .collect();
    let track_len = width - full.min(width) - usize::from(half);
    let track: String = std::iter::repeat_n(BAR_EMPTY, track_len).collect();
    if color {
        format!("{}{filled}{RESET}{DIM}{track}{RESET}", pct_color(pct))
    } else {
        format!("{filled}{track}")
    }
}

/// Every window for one account, or the reason we couldn't get them.
///
/// Live account: read ~/.claude's token as-is and never refresh it — rotating
/// the live login's token family is claude's job, not ours.
pub fn fetch_windows(acct: &Account) -> Result<Vec<Window>> {
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
    Ok(oauth::windows(&oauth::fetch_usage(token)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn bar_width_is_exact_at_every_pct() {
        for pct in [0.0, 1.0, 33.3, 50.0, 76.0, 99.9, 100.0] {
            assert_eq!(
                visible(&bar(pct, 20, true)).chars().count(),
                20,
                "pct {pct}"
            );
            assert_eq!(bar(pct, 20, false).chars().count(), 20, "pct {pct}");
        }
    }

    #[test]
    fn bar_endpoints_are_empty_and_full() {
        assert_eq!(bar(0.0, 5, false), "─────");
        assert_eq!(bar(100.0, 5, false), "━━━━━");
        // Out-of-range percentages clamp instead of overflowing the track.
        assert_eq!(bar(140.0, 5, false), "━━━━━");
    }

    #[test]
    fn severity_ramp_matches_statusline() {
        assert_eq!(pct_color(69.9), GREEN);
        assert_eq!(pct_color(70.0), YELLOW);
        assert_eq!(pct_color(90.0), RED);
    }

    #[test]
    fn pad_counts_visible_chars_only() {
        let styled = format!("{GREEN}ok{RESET}");
        assert_eq!(visible(&pad("ok", &styled, 5)).chars().count(), 5);
    }
}
