//! `cswap list` — one row per account in a borderless table:
//!
//! ```text
//! STATUS   ACCOUNT                ALIAS   USAGE
//! active   work@corp.com          work    5h  96% │ 7d  40%
//! default  devanshw09@gmail.com   main    5h   3% │ 7d  12%
//! ```
//!
//! Deliberately one line per account: the gate percentages at a glance, no
//! reset times, no per-model windows. `cswap usage` is the detailed view.

use anyhow::Result;

use crate::config::{Account, Config};
use crate::ui::{self, DIM, RESET};

pub fn run(quick: bool) -> Result<()> {
    print_table(quick)?;
    crate::update_check::nudge();
    Ok(())
}

struct Row {
    status: String,
    account: String,
    alias: String,
    usage: String,
}

pub fn print_table(quick: bool) -> Result<()> {
    let cfg = Config::load()?;
    if cfg.accounts.is_empty() {
        println!("No accounts yet. Run: cswap login");
        return Ok(());
    }
    let color = ui::color_on();
    let active_email = active_email(&cfg);

    let rows: Vec<Row> = cfg
        .accounts
        .iter()
        .map(|acct| {
            let mut status = Vec::new();
            if active_email.as_deref() == Some(acct.email.as_str()) {
                status.push("active");
            }
            if cfg.default.as_deref() == Some(acct.email.as_str()) {
                status.push("default");
            }
            let usage = if quick {
                String::new()
            } else {
                gates(acct, color)
            };
            Row {
                status: status.join(" "),
                account: acct.email.clone(),
                alias: if acct.aliases.is_empty() {
                    "-".to_string()
                } else {
                    acct.aliases.join(", ")
                },
                usage,
            }
        })
        .collect();

    let w_status = width("STATUS", rows.iter().map(|r| r.status.as_str()));
    let w_account = width("ACCOUNT", rows.iter().map(|r| r.account.as_str()));
    let w_alias = width("ALIAS", rows.iter().map(|r| r.alias.as_str()));

    let header = format!(
        "{:<w_status$}  {:<w_account$}  {:<w_alias$}  {}",
        "STATUS", "ACCOUNT", "ALIAS", "USAGE"
    );
    println!("{}", ui::paint(color, DIM, &header));

    for r in &rows {
        let status = ui::pad(
            &r.status,
            &ui::paint(color, ui::ACCENT, &r.status),
            w_status,
        );
        let alias = ui::pad(&r.alias, &ui::paint(color, DIM, &r.alias), w_alias);
        println!("{status}  {:<w_account$}  {alias}  {}", r.account, r.usage);
    }
    if quick {
        println!("{}", ui::paint(color, DIM, "usage skipped (--quick)"));
    }
    Ok(())
}

/// Which email this shell has activated, resolved through aliases.
pub fn active_email(cfg: &Config) -> Option<String> {
    std::env::var("CSWAP_ACTIVE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|k| cfg.find(&k).map(|a| a.email.clone()).unwrap_or(k))
}

fn width<'a>(header: &str, cells: impl Iterator<Item = &'a str>) -> usize {
    cells
        .map(|c| c.chars().count())
        .chain(std::iter::once(header.chars().count()))
        .max()
        .unwrap_or(0)
}

/// The 5h and 7d gates on one line — per-model windows belong to `cswap usage`.
/// Last column, so it never needs padding: only the styled form is built.
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
