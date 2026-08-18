//! `cswap usage` — the detailed card view. The default card comes first, then
//! one block per profile:
//!
//! ```text
//! default — developer@neuralabs.org  [live ~/.claude]  ● active
//!   5h    ━━━━━╸──────────────   12%  resets 2h 47m · 02:10
//!   7d    ━━━━━━━━━━━━╸───────   30%  resets 5d 3h · 14:00
//!
//!   devanshw09@gmail.com  [wadhwani]
//!   5h    ━━━━━━━━━╸──────────   41%  resets 5d 3h · 14:00
//! ```
//!
//! Same data `cswap list` summarises, with every window, a bar, and reset
//! times. `cswap watch` re-renders this.

use anyhow::Result;

use crate::cmds::list::active_email;
use crate::config::{Config, Target};
use crate::oauth;
use crate::ui::{self, ACCENT, BOLD, DIM, RESET};

/// Bar cells. Wide enough to read a few percent, narrow enough for 80 cols.
const BAR_WIDTH: usize = 24;

pub fn run(key: Option<String>) -> Result<()> {
    let cfg = Config::load()?;
    // Validate the key up front so a typo says so instead of printing nothing.
    let only = match key {
        Some(k) => Some(cfg.resolve_key(&k)?.key().to_string()),
        None => None,
    };
    render(&cfg, only.as_deref());
    crate::update_check::nudge();
    Ok(())
}

/// Print the default card and every profile's card. `only` limits it to one
/// target label (a profile name, or `default`).
pub fn render(cfg: &Config, only: Option<&str>) {
    let color = ui::color_on();
    let active = active_email(cfg);
    let mut first = true;

    if only.is_none_or(|k| k == crate::config::DEFAULT_KEY) {
        if let Some(email) = crate::profile::live_email() {
            let marker = if active.is_none() {
                format!("  {}", ui::paint(color, ACCENT, "● active"))
            } else {
                String::new()
            };
            println!(
                "{} {}  {}{marker}",
                ui::paint(color, DIM, "default —"),
                ui::paint(color, BOLD, &email),
                ui::paint(color, DIM, "[live ~/.claude]")
            );
            for line in card_lines(&Target::Default, color) {
                println!("  {line}");
            }
            first = false;
        }
    }

    for p in cfg.profiles.iter() {
        if only.is_some_and(|k| k != p.email) {
            continue;
        }
        if !first {
            println!();
        }
        first = false;

        let mut tags = String::new();
        if active.as_deref() == Some(p.email.as_str()) {
            tags.push_str(&format!("  {}", ui::paint(color, ACCENT, "● active")));
        }
        let aliases = if p.aliases.is_empty() {
            String::new()
        } else {
            format!(
                "  {}",
                ui::paint(color, DIM, &format!("[{}]", p.aliases.join(", ")))
            )
        };
        println!("  {}{aliases}{tags}", ui::paint(color, BOLD, &p.email));

        for line in card_lines(&Target::Profile(p.clone()), color) {
            println!("  {line}");
        }
    }

    // Codex last, and only in the full view. Absent, logged out, expired or
    // offline all render nothing at all — a cswap user who does not use Codex
    // should never find out this section exists.
    if only.is_none() && crate::codex::is_configured() {
        if let Ok(cx) = crate::codex::usage() {
            if !cx.windows.is_empty() {
                if !first {
                    println!();
                }
                first = false;
                let plan = cx
                    .plan
                    .map(|p| format!("  {}", ui::paint(color, DIM, &format!("[{p}]"))))
                    .unwrap_or_default();
                println!(
                    "{} {}{plan}",
                    ui::paint(color, DIM, "codex —"),
                    ui::paint(color, BOLD, &cx.email)
                );
                for line in render_windows(&cx.windows, color) {
                    println!("  {line}");
                }
            }
        }
    }

    if first {
        println!(
            "{}",
            ui::paint(color, DIM, "Nothing to show. Run: cswap login <email>")
        );
    }
}

fn card_lines(target: &Target, color: bool) -> Vec<String> {
    // A profile that never logged in has no token to spend on a call whose only
    // possible answer is "not logged in".
    if let Target::Profile(p) = target {
        if !crate::profile::has_login(&p.email) {
            return vec![ui::paint(
                color,
                DIM,
                &format!("not logged in — run: cswap login {}", p.email),
            )];
        }
    }
    let windows = match ui::fetch_windows(target) {
        Ok(w) if w.is_empty() => return vec![ui::paint(color, DIM, "no window data")],
        Ok(w) => w,
        Err(e) => return vec![ui::paint(color, DIM, &format!("usage unavailable ({e:#})"))],
    };
    render_windows(&windows, color)
}

/// One padded `label bar pct reset` line per window. Shared so a Codex block
/// and a Claude card line up column for column.
fn render_windows(windows: &[oauth::Window], color: bool) -> Vec<String> {
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
