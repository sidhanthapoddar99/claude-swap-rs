//! The dashboard: what a bare `cswap` opens.

use anyhow::{Context, Result};
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::DefaultTerminal;
use std::io::{stdout, BufRead, IsTerminal, Write};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

pub mod app;
mod pages;
mod theme;
mod ui;
mod widgets;

#[cfg(test)]
mod tests;

use app::{App, Exit, Msg, Suspend};

use crate::manage;
use crate::store::Registry;

/// Open the dashboard. Returns what to do once it has closed, if anything.
pub fn run() -> Result<Option<Exit>> {
    if !stdout().is_terminal() || !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "the dashboard needs a terminal. For text output use `cswap list`, `cswap usage` or `cswap -h`."
        );
    }
    let (tx, rx) = mpsc::channel();
    let mut app = App::new(tx)?;

    // A panic must not leave the terminal reporting mouse movement. ratatui's
    // own hook, added by `init`, puts the rest back.
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        before(info);
    }));

    let mut term = ratatui::init();
    let mut mouse = false;
    let result = main_loop(&mut term, &mut app, &rx, &mut mouse);
    if mouse {
        let _ = execute!(stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    result?;
    Ok(app.exit.take())
}

fn main_loop(
    term: &mut DefaultTerminal,
    app: &mut App,
    rx: &Receiver<Msg>,
    mouse: &mut bool,
) -> Result<()> {
    loop {
        // Mouse reporting follows the setting, also when it changes.
        if app.settings.mouse != *mouse {
            if app.settings.mouse {
                execute!(stdout(), EnableMouseCapture)?;
            } else {
                execute!(stdout(), DisableMouseCapture)?;
            }
            *mouse = app.settings.mouse;
        }
        term.draw(|f| ui::draw(f, app))?;

        // A key or a click wakes the loop at once. With nothing to do it
        // wakes once a second for the clock and the countdowns, and often
        // enough for the spinner while something is being fetched.
        let wait = Duration::from_millis(if app.busy.is_empty() { 1000 } else { 100 });
        if event::poll(wait)? {
            // Take everything that is waiting before drawing again, so a
            // burst of mouse movement costs one frame.
            loop {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => app.on_key(k),
                    Event::Mouse(m) => app.on_mouse(m),
                    _ => {}
                }
                if app.quit || app.suspend.is_some() || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        while let Ok(msg) = rx.try_recv() {
            app.on_msg(msg);
        }
        app.tick();
        if let Some(Suspend::Login { key, created }) = app.suspend.take() {
            let result = hand_off(term, mouse, &key);
            app.after_login(&key, created, result);
        }
        if app.quit {
            return Ok(());
        }
    }
}

/// Give the terminal to the agent's own sign-in, then take it back.
fn hand_off(term: &mut DefaultTerminal, mouse: &mut bool, key: &str) -> Result<bool> {
    if *mouse {
        execute!(stdout(), DisableMouseCapture)?;
        *mouse = false;
    }
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    let _ = term.show_cursor();

    let result = sign_in(key);

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    term.clear()?;
    result
}

fn sign_in(key: &str) -> Result<bool> {
    let mut reg = Registry::load()?;
    let acc = reg.get_mut(key).context("the account is gone")?;
    let mut out = stdout();
    let _ = writeln!(out, "\n{}", manage::sign_in_banner(acc));
    let _ = out.flush();
    let done = manage::sign_in(acc)?;
    if !done {
        // Leave whatever the agent printed on screen until it has been read.
        let _ = writeln!(out, "\ncswap · no login was saved. Press Enter to go back.");
        let _ = out.flush();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
    }
    Ok(done)
}
