//! `cswap list` — the Default (live ~/.claude) on its own line, then one
//! borderless row per registered account:
//!
//! ```text
//! Default  developer@neuralabs.org   not registered   5h 12% │ 7d 30%   ● active
//!
//! STATUS  ACCOUNT               ALIAS        USAGE
//!         devanshw09@gmail.com  wadhwani, 2  5h  3% │ 7d 12%
//! ```
//!
//! The default is derived, not stored: it is whoever is logged into ~/.claude.
//! `STATUS` is `active` only for the account THIS shell activated; with nothing
//! activated the default is what's in effect, so it carries the `● active`.
//! One line per account: the 5h/7d gates only. `cswap usage` is the detail.

use anyhow::Result;

use crate::config::{Account, Config};
use crate::ui::{self, DIM, RESET, YELLOW};

pub fn run(quick: bool) -> Result<()> {
    print_table(quick)?;
    crate::update_check::nudge();
    Ok(())
}

pub fn print_table(quick: bool) -> Result<()> {
    let cfg = Config::load()?;
    let color = ui::color_on();
    let active = active_email(&cfg);

    print_default(&cfg, active.as_deref(), quick, color);
    println!();

    if cfg.accounts.is_empty() {
        println!(
            "{}",
            ui::paint(color, DIM, "No accounts registered. Run: cswap login")
        );
        return Ok(());
    }

    let status_of = |acct: &Account| {
        if active.as_deref() == Some(acct.email.as_str()) {
            "active".to_string()
        } else {
            String::new()
        }
    };
    let aliases_of = |a: &Account| {
        if a.aliases.is_empty() {
            "-".to_string()
        } else {
            a.aliases.join(", ")
        }
    };

    let w_status = width("STATUS", cfg.accounts.iter().map(status_of));
    let w_account = width("ACCOUNT", cfg.accounts.iter().map(|a| a.email.clone()));
    let w_alias = width("ALIAS", cfg.accounts.iter().map(aliases_of));

    let header = format!(
        "{:<w_status$}  {:<w_account$}  {:<w_alias$}  {}",
        "STATUS", "ACCOUNT", "ALIAS", "USAGE"
    );
    println!("{}", ui::paint(color, DIM, &header));

    for acct in &cfg.accounts {
        let status = status_of(acct);
        let status_cell = ui::pad(&status, &ui::paint(color, ui::ACCENT, &status), w_status);
        let aliases = aliases_of(acct);
        let alias_cell = ui::pad(&aliases, &ui::paint(color, DIM, &aliases), w_alias);
        let usage = if quick {
            String::new()
        } else {
            gates(acct, color)
        };
        println!(
            "{status_cell}  {:<w_account$}  {alias_cell}  {usage}",
            acct.email
        );
    }
    if quick {
        println!("{}", ui::paint(color, DIM, "usage skipped (--quick)"));
    }
    Ok(())
}

/// The Default line: live email · registration status · usage · active marker.
fn print_default(cfg: &Config, active: Option<&str>, quick: bool, color: bool) {
    let label = ui::paint(color, DIM, "Default");
    let Some(email) = crate::profile::live_email() else {
        println!(
            "{label}  {}",
            ui::paint(color, DIM, "(nobody logged into ~/.claude — run `claude`)")
        );
        return;
    };
    let reg = if cfg.find(&email).is_some() {
        ui::paint(color, DIM, "registered")
    } else {
        ui::paint(color, YELLOW, "not registered")
    };
    let usage = if quick {
        String::new()
    } else {
        gates(&Account::new(email.clone()), color)
    };
    // With nothing activated, the default is the account actually in effect.
    let marker = if active.is_none() {
        format!("  {}", ui::paint(color, ui::ACCENT, "● active"))
    } else {
        String::new()
    };
    println!("{label}  {email}  {reg}  {usage}{marker}");
}

/// Which email this shell has activated, resolved through aliases.
pub fn active_email(cfg: &Config) -> Option<String> {
    std::env::var("CSWAP_ACTIVE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|k| cfg.find(&k).map(|a| a.email.clone()).unwrap_or(k))
}

fn width(header: &str, cells: impl Iterator<Item = String>) -> usize {
    cells
        .map(|c| c.chars().count())
        .chain(std::iter::once(header.chars().count()))
        .max()
        .unwrap_or(0)
}

/// The 5h and 7d gates on one line — per-model windows belong to `cswap usage`.
fn gates(acct: &Account, color: bool) -> String {
    let windows = match ui::fetch_windows(acct) {
        Ok(w) => w,
        Err(e) => return ui::paint(color, DIM, &format!("unavailable ({e:#})")),
    };
    let mut styled = Vec::new();
    for label in ["5h", "7d"] {
        let Some(w) = windows.iter().find(|w| w.label == label) else {
            continue;
        };
        let text = format!("{label} {:>3.0}%", w.pct);
        styled.push(if color {
            format!(
                "{DIM}{label} {RESET}{}{:>3.0}%{RESET}",
                ui::pct_color(w.pct),
                w.pct
            )
        } else {
            text
        });
    }
    if styled.is_empty() {
        return ui::paint(color, DIM, "no window data");
    }
    styled.join(&ui::paint(color, DIM, " │ "))
}
