//! The inline picker: a short list drawn under the prompt, for
//! `--cswap` and `cswap start`. It draws on stderr, so whatever the
//! agent later prints on stdout stays clean, and it erases itself when done.

use anyhow::{bail, Result};
use chrono::Utc;
use ratatui::crossterm::cursor::{Hide, MoveToColumn, MoveUp, Show};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::queue;
use ratatui::crossterm::terminal::{self, disable_raw_mode, enable_raw_mode, Clear, ClearType};
use std::io::{IsTerminal, Write};

use crate::harness::Harness;
use crate::store::{Account, HarnessSettings, Registry, Settings};
use crate::text::{fit, truncate, width, Paint};
use crate::{manage, model, suggest, view};

pub struct Item {
    /// The row with colour, and the same row without, for measuring.
    pub painted: String,
    pub plain: String,
    pub enabled: bool,
    /// A key that chooses this row at once.
    pub hotkey: Option<char>,
}

/// Puts the terminal back however the picker ends.
struct Raw;

impl Raw {
    fn enter() -> Result<Raw> {
        enable_raw_mode()?;
        Ok(Raw)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

fn draw(out: &mut impl Write, p: &Paint, title: &str, items: &[Item], sel: usize) -> Result<()> {
    let cols = terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .max(20);
    queue!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
    write!(out, "{}\r\n", p.bold(&truncate(title, cols - 1)))?;
    for (i, item) in items.iter().enumerate() {
        let mark = if i == sel {
            p.accent("▸ ")
        } else {
            "  ".to_string()
        };
        let room = cols.saturating_sub(3);
        let row = if !item.enabled {
            p.dim(&truncate(&item.plain, room))
        } else if width(&item.plain) > room {
            truncate(&item.plain, room)
        } else {
            item.painted.clone()
        };
        write!(out, "{mark}{row}\r\n")?;
    }
    let keys = "↑↓ move · Enter choose · Esc cancel";
    write!(out, "{}\r\n", p.dim(&truncate(keys, cols - 1)))?;
    queue!(out, MoveUp(items.len() as u16 + 2))?;
    out.flush()?;
    Ok(())
}

/// Ask the user to choose one row. `None` when they cancel.
pub fn choose(title: &str, items: &[Item], initial: usize) -> Result<Option<usize>> {
    let mut out = std::io::stderr();
    if !out.is_terminal() || !std::io::stdin().is_terminal() {
        bail!("there is no terminal to ask on. Name the account instead: --cswap=<account>");
    }
    if items.is_empty() {
        return Ok(None);
    }
    let p = Paint::stderr();
    let step = |from: usize, delta: i64| -> usize {
        // Move to the next row that can be chosen, staying put at the ends.
        let n = items.len() as i64;
        let mut i = from as i64;
        loop {
            i += delta;
            if i < 0 || i >= n {
                return from;
            }
            if items[i as usize].enabled {
                return i as usize;
            }
        }
    };
    let mut sel = initial.min(items.len() - 1);
    if !items[sel].enabled {
        sel = items.iter().position(|i| i.enabled).unwrap_or(sel);
    }

    let raw = Raw::enter()?;
    queue!(out, Hide)?;
    let chosen = loop {
        draw(&mut out, &p, title, items, sel)?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        match key.code {
            _ if ctrl_c => break None,
            KeyCode::Esc | KeyCode::Char('q') => break None,
            KeyCode::Up | KeyCode::Char('k') => sel = step(sel, -1),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => sel = step(sel, 1),
            KeyCode::Enter if items[sel].enabled => break Some(sel),
            KeyCode::Char(c) => {
                if let Some(i) = items
                    .iter()
                    .position(|it| it.enabled && it.hotkey == Some(c))
                {
                    break Some(i);
                }
            }
            _ => {}
        }
    };
    queue!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown), Show)?;
    out.flush()?;
    drop(raw);
    Ok(chosen)
}

/// One account as a picker row, drawn with `p`. `why_w` is the room left for
/// the reason at the end of the row.
fn account_row(
    acc: &Account,
    hs: &HarnessSettings,
    pick: Option<&suggest::Pick>,
    best: bool,
    signed_in: bool,
    why_w: usize,
    p: &Paint,
) -> String {
    let now = Utc::now();
    let star = if best { "▸" } else { " " };
    let mut title = acc.title();
    if view::is_default(acc, hs) {
        title.push_str(" (default)");
    }
    let name = fit(&title, 22);
    let plan = fit(acc.meta.plan.as_deref().unwrap_or(""), 10);
    let usage = match acc.snapshot().and_then(view::main_window) {
        Some(w) if signed_in => {
            let used = w.used_now(now);
            let reset = match w.resets_at {
                Some(at) if at > now => format!("resets in {}", model::span(at - now)),
                Some(_) => "has reset".to_string(),
                None => String::new(),
            };
            format!(
                "{} {}  {}",
                p.bar(used, 12),
                p.severity(used, &format!("{used:>3.0}%")),
                p.dim(&fit(&reset, 18))
            )
        }
        _ => p.dim(&fit(
            if signed_in {
                "no usage fetched yet"
            } else {
                "not signed in"
            },
            36,
        )),
    };
    let why = truncate(&pick.map(|k| k.headline.clone()).unwrap_or_default(), why_w);
    let why = match pick {
        Some(k) if !k.usable => p.warn(&why),
        _ => p.dim(&why),
    };
    format!(
        "{} {}  {} {} {usage}  {why}",
        p.accent(star),
        p.dim(&format!("{:>2}", acc.meta.id)),
        p.bold(&name),
        p.dim(&plan),
    )
}

fn account_item(
    acc: &Account,
    hs: &HarnessSettings,
    pick: Option<&suggest::Pick>,
    best: bool,
    p: &Paint,
) -> Item {
    let signed_in = acc.harness.logged_in(&acc.config_dir());
    // Everything before the reason has a fixed width; the reason gets the
    // rest of the terminal, so the row never has to be cut.
    let cols = terminal::size().map(|(w, _)| w as usize).unwrap_or(80);
    let why_w = cols.saturating_sub(3 + 78);
    let id = acc.meta.id.to_string();
    Item {
        plain: account_row(acc, hs, pick, best, signed_in, why_w, &Paint::new(false)),
        painted: account_row(acc, hs, pick, best, signed_in, why_w, p),
        enabled: signed_in,
        hotkey: id.chars().next().filter(|_| id.len() == 1),
    }
}

/// Ask which account of `h` to use. Returns its key.
pub fn pick(reg: &Registry, h: Harness, settings: &Settings) -> Result<Option<String>> {
    let accounts: Vec<&Account> = reg.of(h).collect();
    if accounts.is_empty() {
        bail!(
            "there is no {} account. Add one with: cswap add {}",
            h.name(),
            h.id()
        );
    }
    let hs = HarnessSettings::load(h)?;
    let ranked = suggest::rank(&accounts, settings, Utc::now());
    let best = suggest::best(&accounts, settings, Utc::now()).map(|k| k.key);
    let p = Paint::stderr();
    let items: Vec<Item> = accounts
        .iter()
        .map(|acc| {
            let key = acc.key();
            account_item(
                acc,
                &hs,
                ranked.iter().find(|k| k.key == key),
                best.as_deref() == Some(key.as_str()),
                &p,
            )
        })
        .collect();
    if !items.iter().any(|i| i.enabled) {
        bail!(
            "no {} account is signed in. Sign in with: cswap login <account>",
            h.name()
        );
    }
    // Start on the best pick, so Enter alone does the sensible thing.
    let initial = accounts
        .iter()
        .position(|a| Some(a.key()) == best)
        .unwrap_or(0);
    let title = format!("{} · which account?", h.name());
    Ok(choose(&title, &items, initial)?.map(|i| accounts[i].key()))
}

/// Ask which agent to start. Only installed agents can be chosen.
pub fn pick_harness(reg: &Registry) -> Result<Option<Harness>> {
    let p = Paint::stderr();
    let items: Vec<Item> = Harness::ALL
        .into_iter()
        .enumerate()
        .map(|(i, h)| {
            let installed = manage::installed(h);
            let n = reg.of(h).count();
            let hs = HarnessSettings::load(h).unwrap_or_default();
            let default = match hs.start.as_str() {
                "root" => "its root login".to_string(),
                "auto" => "auto".to_string(),
                id => reg
                    .of(h)
                    .find(|a| a.meta.id.to_string() == id)
                    .map(|a| a.title())
                    .unwrap_or_else(|| id.to_string()),
            };
            let detail = if installed {
                format!(
                    "{n} account{} · starts on {default}",
                    if n == 1 { "" } else { "s" }
                )
            } else {
                format!("`{}` is not installed", h.spec().bins[0])
            };
            let num = (i + 1).to_string();
            let name = fit(h.name(), 14);
            let bin = fit(h.spec().bins[0], 8);
            Item {
                plain: format!("{num}  {name} {bin} {detail}"),
                painted: format!(
                    "{}  {} {} {}",
                    p.dim(&num),
                    p.bold(&name),
                    p.info(&bin),
                    p.dim(&detail)
                ),
                enabled: installed,
                hotkey: num.chars().next(),
            }
        })
        .collect();
    if !items.iter().any(|i| i.enabled) {
        bail!("none of claude, codex, agy or grok is installed");
    }
    Ok(choose("Which agent?", &items, 0)?.map(|i| Harness::ALL[i]))
}
