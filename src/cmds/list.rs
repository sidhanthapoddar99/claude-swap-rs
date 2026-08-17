//! `cswap list` — the default on its own line, then one borderless row per
//! profile:
//!
//! ```text
//! default  developer@neuralabs.org  live ~/.claude  5h 12% │ 7d 30%   ● active
//!
//! STATUS  ACCOUNT               ALIAS     USAGE
//!         devanshw09@gmail.com  wadhwani  5h  3% │ 7d 12%
//! ```
//!
//! The default is not a row: it owns no cswap state, so it has no profile
//! directory and no aliases. `STATUS` is `active` only for what THIS shell
//! activated; with nothing activated the default is in effect and carries the
//! marker. One line per profile: the 5h/7d gates only. `cswap usage` is detail.

use anyhow::Result;

use crate::config::{Config, Profile, Target};
use crate::ui::{self, DIM, RESET};

pub fn run(quick: bool) -> Result<()> {
    print_table(quick)?;
    crate::update_check::nudge();
    Ok(())
}

pub fn print_table(quick: bool) -> Result<()> {
    let cfg = Config::load()?;
    let color = ui::color_on();
    let active = active_email(&cfg);

    print_default(active.as_deref(), quick, color);
    println!();

    if cfg.profiles.is_empty() {
        println!(
            "{}",
            ui::paint(color, DIM, "No profiles yet. Run: cswap login <email>")
        );
        return Ok(());
    }

    let status_of = |p: &Profile| {
        if active.as_deref() == Some(p.email.as_str()) {
            "active".to_string()
        } else {
            String::new()
        }
    };
    let aliases_of = |p: &Profile| {
        if p.aliases.is_empty() {
            "-".to_string()
        } else {
            p.aliases.join(", ")
        }
    };

    let w_status = width("STATUS", cfg.profiles.iter().map(status_of));
    let w_account = width("ACCOUNT", cfg.profiles.iter().map(|p| p.email.clone()));
    let w_alias = width("ALIAS", cfg.profiles.iter().map(aliases_of));

    let header = format!(
        "{:<w_status$}  {:<w_account$}  {:<w_alias$}  {}",
        "STATUS", "ACCOUNT", "ALIAS", "USAGE"
    );
    println!("{}", ui::paint(color, DIM, &header));

    for p in &cfg.profiles {
        let status = status_of(p);
        let status_cell = ui::pad(&status, &ui::paint(color, ui::ACCENT, &status), w_status);
        let aliases = aliases_of(p);
        let alias_cell = ui::pad(&aliases, &ui::paint(color, DIM, &aliases), w_alias);
        let usage = if quick {
            String::new()
        } else if !crate::profile::has_login(&p.email) {
            // No token, so no call: the endpoint budgets requests per token and
            // "not logged in" is the answer anyway.
            ui::paint(color, DIM, "not logged in")
        } else {
            gates(&Target::Profile(p.clone()), color)
        };
        println!(
            "{status_cell}  {:<w_account$}  {alias_cell}  {usage}",
            p.email
        );
    }
    if quick {
        println!("{}", ui::paint(color, DIM, "usage skipped (--quick)"));
    }
    Ok(())
}

/// The default line: who ~/.claude is · usage · active marker.
fn print_default(active: Option<&str>, quick: bool, color: bool) {
    let label = ui::paint(color, DIM, "default");
    let Some(email) = crate::profile::live_email() else {
        println!(
            "{label}  {}",
            ui::paint(color, DIM, "(nobody logged into ~/.claude — run `claude`)")
        );
        return;
    };
    let usage = if quick {
        String::new()
    } else {
        gates(&Target::Default, color)
    };
    // With nothing activated, the default is what's actually in effect.
    let marker = if active.is_none() {
        format!("  {}", ui::paint(color, ui::ACCENT, "● active"))
    } else {
        String::new()
    };
    println!(
        "{label}  {email}  {}  {usage}{marker}",
        ui::paint(color, DIM, "live ~/.claude")
    );
}

/// Which account THIS shell activated, resolved through aliases. None means
/// the default is in effect.
pub fn active_email(cfg: &Config) -> Option<String> {
    let key = std::env::var("CSWAP_ACTIVE")
        .ok()
        .filter(|s| !s.is_empty())?;
    if crate::config::is_default_key(&key) {
        return None;
    }
    Some(cfg.find(&key).map(|p| p.email.clone()).unwrap_or(key))
}

fn width(header: &str, cells: impl Iterator<Item = String>) -> usize {
    cells
        .map(|c| c.chars().count())
        .chain(std::iter::once(header.chars().count()))
        .max()
        .unwrap_or(0)
}

/// The 5h and 7d gates on one line — per-model windows belong to `cswap usage`.
fn gates(target: &Target, color: bool) -> String {
    let windows = match ui::fetch_windows(target) {
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
