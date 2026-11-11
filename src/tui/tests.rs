//! The dashboard drawn into a text buffer: every page has to fit its frame
//! and show what it is for, at the smallest size and at a roomy one.

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::sync::mpsc;

use super::app::{App, Focus, Modal, Msg, Page, Suspend};
use super::ui;
use crate::harness::Harness;
use crate::store::Registry;
use crate::testutil::Sandbox;

fn app() -> (App, mpsc::Receiver<Msg>) {
    let (tx, rx) = mpsc::channel();
    (App::new(tx).unwrap(), rx)
}

fn screen(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

/// Print every page when `CSWAP_SHOW=1`, to look at the layout by eye:
/// `CSWAP_SHOW=1 cargo test every_page -- --nocapture`.
fn show(name: &str, text: &str) {
    if std::env::var_os("CSWAP_SHOW").is_some() {
        println!("──── {name}\n{text}");
    }
}

#[test]
fn every_page_draws_at_both_sizes_and_shows_its_content() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    let expect: [(Page, &[&str]); 8] = [
        (
            Page::Overview,
            &["Use next", "work", "Next reset", "Attention"],
        ),
        (Page::Sync, &["work", "linked", "private", "settings.json"]),
        (
            Page::Usage,
            &[
                "week",
                "Opus week",
                "5h window",
                "extra usage",
                "$12.40 of $50.00",
            ],
        ),
        (
            Page::Accounts,
            &["Add account", "Login", "Renewal", "Sign-in", "Launch"],
        ),
        (
            Page::Resets,
            &[
                "Spare resets",
                "weekly reset",
                "Coming up",
                "plan ends",
                "only shows",
            ],
        ),
        (Page::Analytics, &["Burn rate", "PER DAY", "AT RESET"]),
        (Page::Health, &["Checks"]),
        (
            Page::Settings,
            &["Refresh every", "10 min", "General", "Shell"],
        ),
    ];
    for (w, h) in [(ui::MIN_W, ui::MIN_H), (132, 40)] {
        for (page, words) in expect {
            app.dispatch(super::app::Action::Go(page));
            let text = screen(&mut app, w, h);
            show(&format!("{page:?} {w}x{h}"), &text);
            assert!(
                text.contains(page.title()),
                "{page:?} at {w}x{h} has no title"
            );
            if w > ui::MIN_W {
                for word in words {
                    assert!(
                        text.contains(word),
                        "{page:?} at {w}x{h} lacks '{word}'\n{text}"
                    );
                }
            }
        }
    }
    // Too small: say so instead of drawing a broken frame.
    let text = screen(&mut app, 60, 12);
    assert!(text.contains("more room"), "{text}");
}

#[test]
fn the_account_selected_on_the_accounts_page_is_spelled_out() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    app.dispatch(super::app::Action::Go(Page::Accounts));
    let keys = app.account_keys();
    let work = keys
        .iter()
        .position(|k| k == "claude-1")
        .unwrap_or_else(|| panic!("{keys:?}"));
    app.sel[Page::Accounts.index()] = work;
    let text = screen(&mut app, 132, 40);
    show("accounts/work", &text);
    for word in [
        "dev@corp.example",
        "Max 20x",
        "monthly",
        "claude --cswap=work",
        "1 spare",
        "accounts/claude-1",
    ] {
        assert!(text.contains(word), "missing '{word}'\n{text}");
    }
}

#[test]
fn adding_an_account_asks_for_the_agent_then_an_alias_then_hands_over() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    key(&mut app, KeyCode::Char('a'));
    let text = screen(&mut app, 132, 40);
    show("add: agent", &text);
    assert!(
        text.contains("Add an account") && text.contains("Claude Code"),
        "{text}"
    );

    // Codex is the second row.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    let text = screen(&mut app, 132, 40);
    show("add: alias", &text);
    assert!(
        text.contains("New Codex account") && text.contains("Alias (optional)"),
        "{text}"
    );

    // An alias that is taken is refused in place.
    type_text(&mut app, "team");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(
        &app.modal,
        Some(Modal::Input { error: Some(_), .. })
    ));
    for _ in 0..4 {
        key(&mut app, KeyCode::Backspace);
    }
    type_text(&mut app, "night");
    key(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    let Some(Suspend::Login {
        key: new_key,
        created: true,
    }) = app.suspend.take()
    else {
        panic!("the sign-in hand-off was not asked for");
    };
    let reg = Registry::load().unwrap();
    assert_eq!(reg.find(Harness::Codex, "night").unwrap().key(), new_key);

    // The sign-in was abandoned: the empty account is taken away again.
    app.after_login(&new_key, true, Ok(false));
    assert!(Registry::load().unwrap().get(&new_key).is_none());
    assert!(app
        .toast
        .as_ref()
        .is_some_and(|t| t.text.contains("Nothing was added")));
}

#[test]
fn skipping_the_alias_still_creates_the_account() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    key(&mut app, KeyCode::Char('a'));
    key(&mut app, KeyCode::Enter); // Claude Code
    key(&mut app, KeyCode::Enter); // no alias
    let Some(Suspend::Login {
        key: new_key,
        created: true,
    }) = app.suspend.take()
    else {
        panic!("no hand-off");
    };
    // The agent's own sign-in ran and left a login.
    let mut reg = Registry::load().unwrap();
    let done = crate::manage::sign_in(reg.get_mut(&new_key).unwrap());
    app.after_login(&new_key, true, done);
    let reg = Registry::load().unwrap();
    assert_eq!(reg.get(&new_key).unwrap().email(), "new@example.com");
    assert_eq!(app.page, Page::Accounts);
    assert_eq!(app.selected_key().as_deref(), Some(new_key.as_str()));
}

#[test]
fn keys_move_between_the_menu_and_the_page() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    assert_eq!((app.page, app.focus), (Page::Overview, Focus::Nav));
    key(&mut app, KeyCode::Down);
    assert_eq!(app.page, Page::Sync);
    key(&mut app, KeyCode::Char('4'));
    assert_eq!((app.page, app.focus), (Page::Accounts, Focus::Main));
    key(&mut app, KeyCode::Down);
    assert_eq!(app.row(), 1);
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.focus, Focus::Nav);
    key(&mut app, KeyCode::Char('?'));
    assert!(matches!(app.modal, Some(Modal::Info { .. })));
    key(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    key(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}

#[test]
fn a_click_lands_on_what_is_drawn_there() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    let text = screen(&mut app, 132, 40);
    // Find the "Usage" entry of the side menu and click it.
    let (row, line) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.chars().take(26).collect::<String>().contains("Usage"))
        .expect("the menu lists Usage");
    let col = line.chars().take_while(|c| *c != 'U').count();
    app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col as u16,
        row: row as u16,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!((app.page, app.focus), (Page::Usage, Focus::Main));
    // The wheel moves the selection.
    app.on_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 60,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.row(), 1);
}

#[test]
fn settings_change_on_disk() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    key(&mut app, KeyCode::Char('8'));
    key(&mut app, KeyCode::Right); // refresh every: 10 -> 15
    assert_eq!(crate::store::Settings::load().unwrap().refresh_minutes, 15);
    key(&mut app, KeyCode::Char(']')); // the Claude Code tab
    let text = screen(&mut app, 132, 40);
    show("settings/claude", &text);
    assert!(
        text.contains("Wrap `claude`") && text.contains("Default account"),
        "{text}"
    );
    // Row 3 is the default account: step it from the native login to auto.
    for _ in 0..3 {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Right);
    let hs = crate::store::HarnessSettings::load(Harness::Claude).unwrap();
    assert_eq!(hs.default, "auto");
}
