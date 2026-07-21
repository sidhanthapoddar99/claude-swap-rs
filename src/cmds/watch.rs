//! `cswap watch` — `cswap usage`, re-rendered on an interval.
//!
//! Default 300s: the usage endpoint budgets roughly 20-30 requests/hour per
//! token, so with several accounts a 5-minute cadence stays well clear. `r`
//! forces a refresh in between; `q`/Esc/Ctrl-C quit.
//!
//! Keys are read on a helper thread so the main loop can block on "either a
//! keypress or the interval, whichever comes first". On a non-terminal
//! (piped, CI) there is no reader thread and it degrades to a plain sleep.

use anyhow::Result;
use chrono::Local;
use dialoguer::console::{Key, Term};
use std::io::{self, Write};
use std::sync::mpsc;
use std::time::Duration;

use crate::cmds::usage;
use crate::config::Config;
use crate::ui::{self, DIM};

enum Ev {
    Refresh,
    Quit,
}

pub fn run(interval: u64) -> Result<()> {
    let interval = Duration::from_secs(interval.max(60)); // never hammer the API
    let term = Term::stdout();
    let rx = spawn_reader(&term);

    loop {
        print!("\x1b[2J\x1b[H"); // clear + home
        let color = ui::color_on();
        println!(
            "cswap watch {}",
            ui::paint(
                color,
                DIM,
                &format!(
                    "— {}  ·  refresh {}s  ·  [r] now  [q] quit",
                    Local::now().format("%H:%M:%S"),
                    interval.as_secs()
                )
            )
        );
        println!();
        match Config::load() {
            Ok(cfg) if cfg.accounts.is_empty() => println!("No accounts yet. Run: cswap login"),
            Ok(cfg) => usage::render(&cfg, None),
            Err(e) => println!("error: {e:#}"),
        }
        io::stdout().flush()?;

        match &rx {
            Some(rx) => match rx.recv_timeout(interval) {
                Ok(Ev::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Ok(Ev::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            },
            None => std::thread::sleep(interval),
        }
    }
}

/// Reader thread: `r` refreshes, `q`/Esc/Ctrl-C quit, everything else is
/// ignored. Returns None when stdout isn't a terminal (nothing to read).
fn spawn_reader(term: &Term) -> Option<mpsc::Receiver<Ev>> {
    if !term.is_term() {
        return None;
    }
    let term = term.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || loop {
        let ev = match term.read_key() {
            Ok(Key::Char('r')) | Ok(Key::Char('R')) => Ev::Refresh,
            Ok(Key::Char('q')) | Ok(Key::Char('Q')) | Ok(Key::Escape) | Ok(Key::CtrlC) => Ev::Quit,
            // A read error means the terminal went away — stop reading and let
            // the main loop see the disconnect.
            Err(_) => return,
            _ => continue,
        };
        let quit = matches!(ev, Ev::Quit);
        if tx.send(ev).is_err() || quit {
            return;
        }
    });
    Some(rx)
}
