//! `cswap usage` — the detailed card view. The Default (live ~/.claude) card
//! comes first, then one block per registered account:
//!
//! ```text
//! Default — developer@neuralabs.org  [not registered]  ● active
//!   5h    ━━━━━╸──────────────   12%  resets 2h 47m · 02:10
//!   7d    ━━━━━━━━━━━━╸───────   30%  resets 5d 3h · 14:00
//!
//!   devanshw09@gmail.com  [wadhwani, 2]
//!   5h    ━━━━━━━━━╸──────────   41%  resets 5d 3h · 14:00
//! ```
//!
//! Same data `cswap list` summarises, with every window, a bar, and reset
//! times. The default is derived (whoever is live in ~/.claude) and carries
//! `● active` when nothing is activated. `cswap watch` re-renders this.

use anyhow::Result;

use crate::cmds::list::active_email;
use crate::config::{Account, Config};
use crate::oauth;
use crate::ui::{self, ACCENT, BOLD, DIM, RESET, YELLOW};

/// Bar cells. Wide enough to read a few percent, narrow enough for 80 cols.
const BAR_WIDTH: usize = 24;

pub fn run(key: Option<String>) -> Result<()> {
    let cfg = Config::load()?;
    let only = match key {
        Some(k) => Some(
            cfg.find(&k)
                .ok_or_else(|| anyhow::anyhow!("no account matches '{k}'"))?
                .email
                .clone(),
        ),
        None => None,
    };
    render(&cfg, only.as_deref());
    crate::update_check::nudge();
    Ok(())
}

/// Print the Default card and every account's card. `only` limits it to one
/// email (and, when it doesn't match the live login, hides the Default card).
pub fn render(cfg: &Config, only: Option<&str>) {
    let color = ui::color_on();
    let active = active_email(cfg);
    let mut first = true;

    // Default card — the live ~/.claude login, registered or not.
    if let Some(email) = crate::profile::live_email() {
        if only.is_none_or(|e| e == email) {
            let reg = if cfg.find(&email).is_some() {
                ui::paint(color, DIM, "[registered]")
            } else {
                ui::paint(color, YELLOW, "[not registered]")
            };
            let marker = if active.is_none() {
                format!("  {}", ui::paint(color, ACCENT, "● active"))
            } else {
                String::new()
            };
            println!(
                "{} {}  {reg}{marker}",
                ui::paint(color, DIM, "Default —"),
                ui::paint(color, BOLD, &email)
            );
            for line in card_lines(&Account::new(email), color) {
                println!("  {line}");
            }
            first = false;
        }
    }

    for acct in cfg.accounts.iter() {
        if only.is_some_and(|e| e != acct.email) {
            continue;
        }
        if !first {
            println!();
        }
        first = false;

        let mut tags = String::new();
        if active.as_deref() == Some(acct.email.as_str()) {
            tags.push_str(&format!("  {}", ui::paint(color, ACCENT, "● active")));
        }
        if acct.isolated {
            tags.push_str(&format!("  {}", ui::paint(color, DIM, "● isolated")));
        }
        let aliases = if acct.aliases.is_empty() {
            String::new()
        } else {
            format!(
                "  {}",
                ui::paint(color, DIM, &format!("[{}]", acct.aliases.join(", ")))
            )
        };
        println!("  {}{aliases}{tags}", ui::paint(color, BOLD, &acct.email));

        for line in card_lines(acct, color) {
            println!("  {line}");
        }
    }

    if first {
        println!(
            "{}",
            ui::paint(color, DIM, "Nothing to show. Run: cswap login")
        );
    }
}

fn card_lines(acct: &Account, color: bool) -> Vec<String> {
    let windows = match ui::fetch_windows(acct) {
        Ok(w) if w.is_empty() => return vec![ui::paint(color, DIM, "no window data")],
        Ok(w) => w,
        Err(e) => return vec![ui::paint(color, DIM, &format!("usage unavailable ({e:#})"))],
    };
    let label_w = windows
        .iter()
        .map(|w| w.label.chars().count())
        .max()
        .unwrap_or(2);

    windows
        .iter()
        .map(|w| {
            let label = ui::pad(&w.label, &ui::paint(color, DIM, &w.label), label_w);
            let pct = if color {
                format!("{}{:>3.0}%{RESET}", ui::pct_color(w.pct), w.pct)
            } else {
                format!("{:>3.0}%", w.pct)
            };
            let reset = w
                .resets_at
                .as_deref()
                .and_then(oauth::reset_detail)
                .map(|r| format!("  {}", ui::paint(color, DIM, &r)))
                .unwrap_or_default();
            format!("{label} {} {pct}{reset}", ui::bar(w.pct, BAR_WIDTH, color))
        })
        .collect()
}
