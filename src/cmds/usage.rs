//! `cswap usage` — the detailed card view, one block per account:
//!
//! ```text
//!   devanshw09@gmail.com  [main]   ● active
//!   5h    ━━━━━╸──────────────   26%  resets 2h 47m · 02:10
//!   7d    ━━━━━━━━━━━━╸───────   59%  resets 5d 3h · 14:00
//!   Fable ━━━━━━━━━╸──────────   41%  resets 5d 3h · 14:00
//! ```
//!
//! Same data `cswap list` summarises, with every window, a bar, and reset
//! times. `cswap watch` re-renders exactly this on an interval.

use anyhow::Result;

use crate::cmds::list::active_email;
use crate::config::Config;
use crate::oauth;
use crate::ui::{self, ACCENT, BOLD, DIM, RESET};

/// Bar cells. Wide enough to read a few percent, narrow enough for 80 cols.
const BAR_WIDTH: usize = 24;

pub fn run(key: Option<String>) -> Result<()> {
    let cfg = Config::load()?;
    if cfg.accounts.is_empty() {
        println!("No accounts yet. Run: cswap login");
        return Ok(());
    }
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

/// Print every account's card. `only` limits it to one email.
pub fn render(cfg: &Config, only: Option<&str>) {
    let color = ui::color_on();
    let active = active_email(cfg);
    let mut first = true;

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
        if cfg.default.as_deref() == Some(acct.email.as_str()) {
            tags.push_str(&format!("  {}", ui::paint(color, DIM, "● default")));
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
}

fn card_lines(acct: &crate::config::Account, color: bool) -> Vec<String> {
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
