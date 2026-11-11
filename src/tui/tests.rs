//! The dashboard drawn into a text buffer: every page has to fit its frame
//! and show what it is for, at the smallest size and at a roomy one.

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::sync::mpsc;

use super::app::{Action, App, Focus, Modal, Msg, Page, Session, Suspend};
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
    let expect: [(Page, &[&str]); 10] = [
        (
            Page::Overview,
            &[
                "Use next",
                "work",
                "Next reset",
                "Spare resets",
                "Attention",
                "RESETS IN",
            ],
        ),
        (
            Page::Usage,
            &[
                "LIMIT",
                "RESETS IN",
                "week",
                "Opus week",
                "extra",
                "$12.40 of $50.00",
            ],
        ),
        (
            Page::Accounts,
            &[
                "Add account",
                "Aliases",
                "Login",
                "Renewal",
                "Sign-in",
                "Launch",
            ],
        ),
        (
            Page::Defaults,
            &[
                "AGENT",
                "LOGIN",
                "dev@example.com",
                "Max 5x",
                "SuperGrok",
                "default login",
            ],
        ),
        (Page::Sessions, &["Nothing is running"]),
        (
            Page::Resets,
            &["Coming up", "Spare resets", "Weekly", "Renewals", "TYPE"],
        ),
        (Page::Sync, &["work", "linked", "private", "settings.json"]),
        (Page::Analytics, &["Burn rate", "PER DAY", "AT RESET"]),
        (Page::Health, &["Checks"]),
        (
            Page::Settings,
            &[
                "Theme",
                "violet",
                "Refresh every",
                "10 min",
                "General",
                "Shell",
            ],
        ),
    ];
    for (w, h) in [(ui::MIN_W, ui::MIN_H), (132, 40)] {
        for (page, words) in expect {
            app.dispatch(Action::Go(page));
            let text = screen(&mut app, w, h);
            show(&format!("{page:?} {w}x{h}"), &text);
            assert!(
                text.contains(page.title()),
                "{page:?} at {w}x{h} has no title"
            );
            // Two words that are never shown: the agent's own login is its
            // "default", and the short windows are kept out of sight.
            for word in ["native", "Native", "5h window", "  5h ", "5-hour"] {
                assert!(
                    !text.contains(word),
                    "{page:?} at {w}x{h} says '{word}'\n{text}"
                );
            }
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
fn the_default_logins_are_on_their_own_page_and_nowhere_else() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    // Only cswap's accounts are rows; an agent's own login is not an account.
    assert_eq!(app.account_keys(), ["claude-1", "claude-2", "codex-1"]);
    assert_eq!(
        app.default_keys(),
        ["claude-0", "codex-0", "agy-0", "grok-0"]
    );
    // What only the default logins have: their plans.
    for page in [
        Page::Overview,
        Page::Usage,
        Page::Accounts,
        Page::Resets,
        Page::Sync,
        Page::Analytics,
    ] {
        app.dispatch(Action::Go(page));
        let text = screen(&mut app, 132, 40);
        for word in ["Max 5x", "SuperGrok"] {
            assert!(!text.contains(word), "{page:?} shows '{word}'\n{text}");
        }
    }
    app.dispatch(Action::Go(Page::Defaults));
    let text = screen(&mut app, 132, 40);
    show("defaults", &text);
    for word in ["Claude Code", "Codex", "Antigravity", "Grok", "47%"] {
        assert!(text.contains(word), "missing '{word}'\n{text}");
    }
}

#[test]
fn every_page_holds_a_frame_with_no_accounts_at_all() {
    let sb = Sandbox::new();
    sb.fake_agents();
    for h in Harness::ALL {
        sb.native(h);
    }
    let (mut app, _rx) = app();
    assert!(app.account_keys().is_empty());
    for page in Page::ALL {
        app.dispatch(Action::Go(page));
        for (w, h) in [(ui::MIN_W, ui::MIN_H), (132, 40)] {
            let text = screen(&mut app, w, h);
            assert!(text.contains(page.title()), "{page:?}\n{text}");
        }
    }
    // Every page of accounts says how to get the first one.
    for page in [
        Page::Overview,
        Page::Usage,
        Page::Accounts,
        Page::Resets,
        Page::Analytics,
    ] {
        app.dispatch(Action::Go(page));
        let text = screen(&mut app, 132, 40);
        show(&format!("{page:?}/empty"), &text);
        assert!(
            text.contains("No accounts yet") && text.contains("Press a to add one"),
            "{page:?}\n{text}"
        );
    }
}

#[test]
fn the_account_selected_on_the_accounts_page_is_spelled_out() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    app.dispatch(Action::Go(Page::Accounts));
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
fn an_account_is_renamed_and_given_aliases_from_its_page() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    app.dispatch(Action::Go(Page::Accounts));
    assert_eq!(app.selected_key().as_deref(), Some("claude-1"));

    // e renames. The name cannot be emptied, and cannot be another's.
    key(&mut app, KeyCode::Char('e'));
    for _ in 0..4 {
        key(&mut app, KeyCode::Backspace);
    }
    key(&mut app, KeyCode::Enter);
    assert!(matches!(
        &app.modal,
        Some(Modal::Input { error: Some(_), .. })
    ));
    type_text(&mut app, "side");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(
        &app.modal,
        Some(Modal::Input { error: Some(_), .. })
    ));
    for _ in 0..4 {
        key(&mut app, KeyCode::Backspace);
    }
    type_text(&mut app, "office");
    key(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());

    // E sets the extra names.
    key(&mut app, KeyCode::Char('E'));
    type_text(&mut app, "w, corp");
    key(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none(), "{:?}", app.modal);
    let reg = Registry::load().unwrap();
    let acc = reg.get("claude-1").unwrap();
    assert_eq!(acc.meta.label, "office");
    assert_eq!(acc.meta.aliases, ["w", "corp"]);
    for name in ["office", "w", "corp", "1"] {
        assert_eq!(reg.find(Harness::Claude, name).unwrap().key(), "claude-1");
    }
    let text = screen(&mut app, 132, 40);
    show("accounts/renamed", &text);
    assert!(
        text.contains("office") && text.contains("w, corp"),
        "{text}"
    );
}

#[test]
fn adding_an_account_asks_for_the_agent_then_a_name_then_hands_over() {
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
    show("add: name", &text);
    assert!(
        text.contains("New Codex account") && text.contains("Name"),
        "{text}"
    );

    // No name is no account: every account has one.
    key(&mut app, KeyCode::Enter);
    assert!(matches!(
        &app.modal,
        Some(Modal::Input { error: Some(_), .. })
    ));
    assert!(app.suspend.is_none());
    // A name that is taken is refused in place.
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

/// Press `c` on the Claude row of the Defaults page, accept the name on
/// offer, and let the fake agent sign in. Returns the new account's key.
fn account_from_default(app: &mut App) -> String {
    app.dispatch(Action::Go(Page::Defaults));
    assert_eq!(app.selected_key().as_deref(), Some("claude-0"));
    key(app, KeyCode::Char('c'));
    key(app, KeyCode::Enter);
    let Some(Suspend::Login {
        key: new_key,
        created: true,
    }) = app.suspend.take()
    else {
        panic!("no hand-off: {:?}", app.modal);
    };
    let mut reg = Registry::load().unwrap();
    let done = crate::manage::sign_in(reg.get_mut(&new_key).unwrap());
    app.after_login(&new_key, true, done);
    new_key
}

#[test]
fn a_default_login_becomes_an_account_by_naming_it_and_signing_in_again() {
    let sb = Sandbox::new();
    sb.demo();
    sb.fake_agents();
    let (mut app, _rx) = app();
    app.dispatch(Action::Go(Page::Defaults));
    key(&mut app, KeyCode::Char('c'));
    let text = screen(&mut app, 132, 40);
    show("defaults: make account", &text);
    // The name on offer comes from the login's email, and nothing is copied.
    assert!(
        text.contains("default login") && text.contains("never copies a login"),
        "{text}"
    );
    assert!(
        matches!(&app.modal, Some(Modal::Input { value, .. }) if value == "dev"),
        "{:?}",
        app.modal
    );
    key(&mut app, KeyCode::Esc);

    let new_key = account_from_default(&mut app);
    let reg = Registry::load().unwrap();
    let acc = reg.get(&new_key).unwrap();
    assert_eq!(acc.meta.label, "dev");
    assert_eq!(acc.email(), "new@example.com");
    // Its login is its own: nothing of the default's was copied into it.
    assert!(!acc.is_native());
    assert_eq!(app.page, Page::Accounts);
    assert_eq!(app.selected_key().as_deref(), Some(new_key.as_str()));
}

#[test]
fn a_login_can_be_held_by_one_account_only() {
    let sb = Sandbox::new();
    sb.demo();
    sb.fake_agents();
    let (mut app, _rx) = app();
    // The fake agent always signs in as the same person.
    let first = account_from_default(&mut app);
    assert!(Registry::load().unwrap().get(&first).is_some());

    key(&mut app, KeyCode::Char('a'));
    key(&mut app, KeyCode::Enter); // Claude Code
    type_text(&mut app, "again");
    key(&mut app, KeyCode::Enter);
    let Some(Suspend::Login {
        key: second,
        created: true,
    }) = app.suspend.take()
    else {
        panic!("no hand-off");
    };
    let mut reg = Registry::load().unwrap();
    let done = crate::manage::sign_in(reg.get_mut(&second).unwrap());
    app.after_login(&second, true, done);

    let reg = Registry::load().unwrap();
    assert!(reg.get(&first).is_some());
    assert!(reg.get(&second).is_none(), "the second account was kept");
    assert!(
        app.toast
            .as_ref()
            .is_some_and(|t| t.text.contains("already holds that login")),
        "{:?}",
        app.toast.as_ref().map(|t| &t.text)
    );
}

#[test]
fn the_sessions_page_lists_what_is_running_and_where() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    let now = chrono::Utc::now();
    app.sessions = vec![
        Session {
            key: "claude-1".into(),
            pid: 4242,
            cwd: Some("/srv/api".into()),
            started: Some(now - chrono::Duration::hours(2)),
        },
        Session {
            key: "codex-0".into(),
            pid: 77,
            cwd: None,
            started: None,
        },
    ];
    app.dispatch(Action::Go(Page::Sessions));
    // Going to the page looks again; put the made-up sessions back.
    let made_up = app.sessions.is_empty();
    assert!(made_up || app.sessions.len() == 2);
    app.sessions = vec![
        Session {
            key: "claude-1".into(),
            pid: 4242,
            cwd: Some("/srv/api".into()),
            started: Some(now - chrono::Duration::hours(2)),
        },
        Session {
            key: "codex-0".into(),
            pid: 77,
            cwd: None,
            started: None,
        },
    ];
    let text = screen(&mut app, 132, 40);
    show("sessions", &text);
    for word in [
        "2 running",
        "work",
        "4242",
        "2h",
        "/srv/api",
        "Codex",
        "default",
        "77",
    ] {
        assert!(text.contains(word), "missing '{word}'\n{text}");
    }
    // Nothing about sessions leaks onto the other pages.
    app.dispatch(Action::Go(Page::Overview));
    let text = screen(&mut app, 132, 40);
    assert!(
        !text.contains("Running now") && !text.contains("4242"),
        "{text}"
    );
}

#[test]
fn keys_move_between_the_menu_and_the_page() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    assert_eq!((app.page, app.focus), (Page::Overview, Focus::Nav));
    key(&mut app, KeyCode::Down);
    assert_eq!(app.page, Page::Usage);
    key(&mut app, KeyCode::Char('3'));
    assert_eq!((app.page, app.focus), (Page::Accounts, Focus::Main));
    key(&mut app, KeyCode::Down);
    assert_eq!(app.row(), 1);
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.focus, Focus::Nav);
    // Ten pages: 1 to 9, then 0 for the last.
    key(&mut app, KeyCode::Char('4'));
    assert_eq!(app.page, Page::Defaults);
    key(&mut app, KeyCode::Char('5'));
    assert_eq!(app.page, Page::Sessions);
    key(&mut app, KeyCode::Char('0'));
    assert_eq!(app.page, Page::Settings);
    key(&mut app, KeyCode::Char('?'));
    assert!(matches!(app.modal, Some(Modal::Info { .. })));
    key(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    // Escape steps back, and from the menu it quits. `q` is not a key.
    key(&mut app, KeyCode::Char('q'));
    assert!(!app.quit);
    key(&mut app, KeyCode::Char('3'));
    key(&mut app, KeyCode::Esc);
    assert_eq!((app.focus, app.quit), (Focus::Nav, false));
    key(&mut app, KeyCode::Esc);
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
    // The key hints at the bottom are clickable too.
    let text = screen(&mut app, 132, 40);
    let (row, line) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("? keys"))
        .expect("the footer lists the help key");
    let col = line.split("? keys").next().unwrap().chars().count();
    app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col as u16,
        row: row as u16,
        modifiers: KeyModifiers::NONE,
    });
    assert!(matches!(app.modal, Some(Modal::Info { .. })));
}

#[test]
fn settings_change_on_disk() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    key(&mut app, KeyCode::Char('0'));
    key(&mut app, KeyCode::Down); // past the theme
    key(&mut app, KeyCode::Right); // refresh every: 10 -> 15
    assert_eq!(crate::store::Settings::load().unwrap().refresh_minutes, 15);
    key(&mut app, KeyCode::Char(']')); // the Claude Code tab
    let text = screen(&mut app, 132, 40);
    show("settings/claude", &text);
    assert!(
        text.contains("Wrap `claude`") && text.contains("A bare `claude` starts on"),
        "{text}"
    );
    assert!(text.contains("its default login"), "{text}");
    // Row 3 is where a bare launch starts: step it from the default login
    // to auto, then on to the first account.
    for _ in 0..3 {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Right);
    let hs = crate::store::HarnessSettings::load(Harness::Claude).unwrap();
    assert_eq!(hs.start, "auto");
    key(&mut app, KeyCode::Right);
    let hs = crate::store::HarnessSettings::load(Harness::Claude).unwrap();
    assert_eq!(hs.start, "1");
    let text = screen(&mut app, 132, 40);
    assert!(text.contains("account work"), "{text}");
}

#[test]
fn the_theme_is_switched_in_settings_and_every_theme_draws_every_page() {
    use super::theme::Scheme;
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    assert_eq!(app.theme.scheme(), Scheme::Violet);
    key(&mut app, KeyCode::Char('0'));
    // The theme is the first row. Left of the first theme is the last one.
    key(&mut app, KeyCode::Left);
    assert_eq!(app.theme.scheme(), Scheme::Terminal);
    assert_eq!(crate::store::Settings::load().unwrap().theme, "terminal");
    for expected in Scheme::ALL {
        key(&mut app, KeyCode::Right);
        assert_eq!(app.theme.scheme(), expected);
        assert_eq!(
            crate::store::Settings::load().unwrap().theme,
            expected.name()
        );
        let text = screen(&mut app, 132, 40);
        assert!(text.contains(expected.name()), "{text}");
        for page in Page::ALL {
            app.dispatch(Action::Go(page));
            let text = screen(&mut app, ui::MIN_W, ui::MIN_H);
            assert!(text.contains(page.title()), "{expected:?} {page:?}");
        }
        app.dispatch(Action::Go(Page::Settings));
    }
    // A name nobody knows, typed into the file by hand, is the first theme.
    crate::store::Settings::update(|s| s.theme = "plaid".into()).unwrap();
    app.reload();
    assert_eq!(app.theme.scheme(), Scheme::Violet);
}

/// `cargo test readme_picture -- --ignored --nocapture` prints the Overview
/// at the size of the picture at the top of the README.
#[test]
#[ignore = "prints the README's picture"]
fn readme_picture() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    app.dispatch(Action::Go(Page::Overview));
    println!("{}", screen(&mut app, 116, 24));
}
