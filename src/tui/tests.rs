//! The dashboard drawn into a text buffer: every page has to fit its frame
//! and show what it is for, at the smallest size and at a roomy one.

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::sync::mpsc;

use super::app::{Action, App, Focus, Modal, Msg, Page, Session, Suspend, UpdateCheck};
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
    let expect: [(Page, &[&str]); 12] = [
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
                "Claude Code",
                "Codex",
                "LIMIT",
                "USED",
                "DATE",
                "Ultra",
                "week",
                "Opus",
                "5h",
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
                "Usage",
                "Resets",
                "Details",
            ],
        ),
        (
            Page::Root,
            &[
                "AGENT",
                "LOGIN",
                "dev@example.com",
                "Max",
                "SuperGrok",
                "root login",
            ],
        ),
        (Page::Sessions, &["Nothing is running"]),
        (
            Page::Resets,
            &[
                "Spare resets",
                "Weekly resets",
                "Renewals",
                "COUNT",
                "RESET",
                "EXPIRES",
                "DATE",
                "USED",
                "PLAN",
            ],
        ),
        (Page::Sync, &["work", "linked", "private", "settings.json"]),
        (Page::Analytics, &["Burn rate", "PER DAY", "AT RESET"]),
        (Page::Health, &["Checks"]),
        (
            Page::Integrations,
            &[
                "herdr",
                "not part of the binary",
                "What it adds",
                "same conversation",
                "herdr is not on this machine",
            ],
        ),
        (
            Page::Settings,
            &[
                "Theme",
                "violet",
                "Refresh every",
                "10 min",
                "General",
                "Harness",
                "Shell",
                "Check for updates now",
            ],
        ),
        (
            Page::Help,
            &[
                "STARTING AN AGENT",
                "claude --cswap ",
                "pick the account from a list",
                "claude --cswap=<account>",
                "cswap claude",
                "export CSWAP_CLAUDE=work",
                "WORDS",
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
            // The agent's own login is its "root", never "native". The short
            // windows are listed on the Usage page and in an account's own
            // Usage card, and nowhere else.
            let short = if matches!(page, Page::Usage | Page::Accounts) {
                "5-hour"
            } else {
                "  5h "
            };
            for word in ["native", "Native", "5h window", "5-hour", short] {
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
fn the_root_logins_are_on_their_own_page_and_nowhere_else() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    // Only cswap's accounts are rows; an agent's own login is not an account.
    assert_eq!(app.account_keys(), ["claude-1", "claude-2", "codex-1"]);
    assert_eq!(app.root_keys(), ["claude-0", "codex-0", "agy-0", "grok-0"]);
    // What only the root logins have: a login and a plan of their own.
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
        for word in ["dev@example.com", "SuperGrok"] {
            assert!(!text.contains(word), "{page:?} shows '{word}'\n{text}");
        }
    }
    app.dispatch(Action::Go(Page::Root));
    let text = screen(&mut app, 132, 40);
    show("root", &text);
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
        "Ultra",
        "claude --cswap=work",
        "accounts/claude-1",
        // Every limit it has, the short one too, and its credits.
        "╭ Usage",
        "LIMIT",
        "week",
        "Opus",
        "5h",
        "$12.40 of $50.00 spent",
        // The resets it holds, in a part of their own.
        "1 to spend",
        "COUNT",
        "weekly",
        // The wordy part comes last.
        "Details",
        "monthly · set by hand",
        "Access token lasts",
    ] {
        assert!(text.contains(word), "missing '{word}'\n{text}");
    }
    // The facts stay a line each: the renewal is a date and a count of days.
    let renewal = text
        .lines()
        .find(|l| l.contains("Renewal") && l.contains("in 12 days"))
        .unwrap_or_else(|| panic!("no renewal line\n{text}"));
    assert!(
        !renewal.contains("monthly") && !renewal.contains("press"),
        "{renewal}"
    );
    // The four parts, in order, with the add button on the last line of the list.
    let at = |word: &str| {
        text.find(word)
            .unwrap_or_else(|| panic!("no '{word}'\n{text}"))
    };
    assert!(at("Login") < at("╭ Usage") && at("╭ Usage") < at("╭ Resets"));
    assert!(at("╭ Resets") < at("╭ Details"));
    let rows: Vec<&str> = text.lines().collect();
    let add = rows
        .iter()
        .position(|l| l.contains("+ Add account"))
        .unwrap();
    assert!(rows[add + 1].contains('╰'), "{text}");
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
fn account_from_root(app: &mut App) -> String {
    app.dispatch(Action::Go(Page::Root));
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
fn a_root_login_becomes_an_account_by_naming_it_and_signing_in_again() {
    let sb = Sandbox::new();
    sb.demo();
    sb.fake_agents();
    let (mut app, _rx) = app();
    app.dispatch(Action::Go(Page::Root));
    key(&mut app, KeyCode::Char('c'));
    let text = screen(&mut app, 132, 40);
    show("root: make account", &text);
    // The name on offer comes from the login's email, and nothing is copied.
    assert!(
        text.contains("root login") && text.contains("never copies a login"),
        "{text}"
    );
    assert!(
        matches!(&app.modal, Some(Modal::Input { value, .. }) if value == "dev"),
        "{:?}",
        app.modal
    );
    key(&mut app, KeyCode::Esc);

    let new_key = account_from_root(&mut app);
    let reg = Registry::load().unwrap();
    let acc = reg.get(&new_key).unwrap();
    assert_eq!(acc.meta.label, "dev");
    assert_eq!(acc.email(), "new@example.com");
    // Its login is its own: nothing of the root's was copied into it.
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
    let first = account_from_root(&mut app);
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
        "root",
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
    key(&mut app, KeyCode::Char('4'));
    assert_eq!((app.page, app.focus), (Page::Accounts, Focus::Main));
    key(&mut app, KeyCode::Down);
    assert_eq!(app.row(), 1);
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.focus, Focus::Nav);
    // The pages in the order of the menu: 1 to 9, i for Integrations, 0 for
    // Settings and ? for Help.
    let keys: String = Page::ALL.iter().map(|p| p.hotkey()).collect();
    assert_eq!(keys, "123456789i0?");
    key(&mut app, KeyCode::Char('3'));
    assert_eq!(app.page, Page::Resets);
    key(&mut app, KeyCode::Char('5'));
    assert_eq!(app.page, Page::Root);
    key(&mut app, KeyCode::Char('7'));
    assert_eq!(app.page, Page::Sessions);
    key(&mut app, KeyCode::Char('i'));
    assert_eq!((app.page, app.focus), (Page::Integrations, Focus::Main));
    key(&mut app, KeyCode::Char('0'));
    assert_eq!(app.page, Page::Settings);
    // ? opens Help from anywhere, and a second ? goes back.
    key(&mut app, KeyCode::Char('?'));
    assert_eq!((app.page, app.focus), (Page::Help, Focus::Main));
    key(&mut app, KeyCode::Char('?'));
    assert_eq!(app.page, Page::Settings);
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
fn a_page_is_named_by_its_title_in_any_case() {
    for page in Page::ALL {
        assert_eq!(Page::named(&page.title().to_lowercase()), Some(page));
    }
    assert_eq!(Page::named(" Usage "), Some(Page::Usage));
    assert_eq!(Page::named("usages"), None);
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
        .find(|(_, l)| l.contains("? help"))
        .expect("the footer lists the help key");
    let col = line.split("? help").next().unwrap().chars().count();
    app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col as u16,
        row: row as u16,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.page, Page::Help);
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

    // One tab holds every agent's options, a group each, after the ones
    // they share. Where a launch starts is not set here.
    key(&mut app, KeyCode::Char(']'));
    let text = screen(&mut app, 132, 60);
    show("settings/harness", &text);
    // The keys that change tab are written beside the tabs: t and T, and
    // the brackets do the same.
    assert!(text.contains("t next tab · T previous"), "{text}");
    key(&mut app, KeyCode::Char('t'));
    assert_eq!(app.settings_tab, 2);
    key(&mut app, KeyCode::Char('t'));
    assert_eq!(app.settings_tab, 0);
    key(&mut app, KeyCode::Char('T'));
    assert_eq!(app.settings_tab, 2);
    key(&mut app, KeyCode::Char('['));
    assert_eq!(app.settings_tab, 1);
    for word in [
        "ALL AGENTS",
        "Auto-pick skips above",
        "CLAUDE CODE",
        "CODEX",
        "ANTIGRAVITY",
        "GROK BUILD",
        "Auto-pick the account",
        "Read spare resets",
    ] {
        assert!(text.contains(word), "missing '{word}'\n{text}");
    }
    assert!(
        !text.contains("starts on") && !text.contains("Wrap `"),
        "{text}"
    );

    // Auto-pick is a switch: on stores `auto`, off goes back to the root login.
    let rows = crate::tui::pages::settings::rows(&app);
    let at = rows
        .iter()
        .position(|r| r.label == "Auto-pick the account")
        .unwrap();
    app.sel[Page::Settings.index()] = at;
    let start = || {
        crate::store::HarnessSettings::load(Harness::Claude)
            .unwrap()
            .start
    };
    key(&mut app, KeyCode::Enter);
    assert_eq!(start(), "auto");
    key(&mut app, KeyCode::Enter);
    assert_eq!(start(), "root");

    // The list is longer than a short screen: the selection stays in view.
    app.sel[Page::Settings.index()] = rows.len() - 1;
    let text = screen(&mut app, ui::MIN_W, ui::MIN_H);
    assert!(text.contains("GROK BUILD"), "{text}");
}

#[test]
fn an_update_check_can_be_asked_for_and_says_what_it_found() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, rx) = app();
    key(&mut app, KeyCode::Char('0'));
    let rows = crate::tui::pages::settings::rows(&app);
    let at = rows
        .iter()
        .position(|r| r.label == "Check for updates now")
        .unwrap();
    app.sel[Page::Settings.index()] = at;
    let have = env!("CARGO_PKG_VERSION");
    assert!(screen(&mut app, 132, 40).contains(&format!("you have {have}")));

    // The tests run offline: the check says it could not tell, and never
    // that this is the newest.
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.update, UpdateCheck::Checking);
    let msg = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the check never answered");
    app.on_msg(msg);
    assert!(
        matches!(app.update, UpdateCheck::Failed(_)),
        "{:?}",
        app.update
    );
    let text = screen(&mut app, 132, 40);
    assert!(text.contains("could not check"), "{text}");
    assert!(text.contains("offline"), "{text}");

    // What the two answers look like.
    app.on_msg(Msg::UpdateChecked(Ok(None)));
    let text = screen(&mut app, 132, 40);
    assert!(text.contains(&format!("{have} is the newest")), "{text}");
    app.on_msg(Msg::UpdateChecked(Ok(Some("99.0.0".into()))));
    let text = screen(&mut app, 132, 40);
    assert!(text.contains("99.0.0 is out"), "{text}");
    assert!(text.contains("cswap update"), "{text}");
}

#[cfg(unix)]
#[test]
fn path_shims_are_turned_on_and_off_from_the_shell_tab() {
    let sb = Sandbox::new();
    sb.demo();
    sb.fake_agents();
    let (mut app, _rx) = app();
    key(&mut app, KeyCode::Char('0'));
    key(&mut app, KeyCode::Char(']'));
    key(&mut app, KeyCode::Char(']'));
    let text = screen(&mut app, 132, 40);
    assert!(text.contains("OTHER PROGRAMS"), "{text}");
    let row = text.lines().find(|l| l.contains("PATH shims   ")).unwrap();
    assert!(row.contains("off"), "{row}");

    // The row comes after the shells. Enter opens the boxes, none ticked.
    let at = app.derived.shells.len();
    app.sel[Page::Settings.index()] = at;
    key(&mut app, KeyCode::Enter);
    let text = screen(&mut app, 132, 40);
    show("settings/shims dialog", &text);
    assert!(text.contains("Agents with a PATH shim"), "{text}");
    assert!(text.contains("[ ] claude"), "{text}");
    key(&mut app, KeyCode::Char(' '));
    key(&mut app, KeyCode::Enter);

    // Saved: the link, the line in the login shell's file, and what to do next.
    let text = screen(&mut app, 132, 40);
    show("settings/shims on", &text);
    assert!(text.contains("PATH shims are on for claude"), "{text}");
    assert!(
        text.contains("Added the PATH line to ~/.zprofile"),
        "{text}"
    );
    assert!(text.contains("hc server restart --fresh"), "{text}");
    assert_eq!(crate::store::Settings::load().unwrap().shims, ["claude"]);
    let link = crate::shim::dir().join("claude");
    assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    let zprofile = crate::paths::home().join(".zprofile");
    assert!(std::fs::read_to_string(&zprofile)
        .unwrap()
        .contains(".cswap/shims"));
    key(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none());
    let text = screen(&mut app, 132, 40);
    let row = text.lines().find(|l| l.contains("PATH shims   ")).unwrap();
    assert!(row.contains("claude"), "{row}");

    // The shim covers every shell, so each shell shows claude as integrated
    // and its tick there is locked, with the reason beside it.
    let zsh = app
        .derived
        .shells
        .iter()
        .position(|s| s.name == "zsh")
        .unwrap();
    assert!(app.derived.shells[zsh].wraps(Harness::Claude));
    assert!(!app.derived.shells[zsh].wraps(Harness::Codex));
    app.sel[Page::Settings.index()] = zsh;
    let text = screen(&mut app, 132, 40);
    show("settings/shell with a shim", &text);
    assert!(text.contains("tick here is locked"), "{text}");
    key(&mut app, KeyCode::Enter);
    let text = screen(&mut app, 132, 40);
    show("settings/shell dialog with a shim", &text);
    assert!(text.contains("PATH shim is on"), "{text}");
    key(&mut app, KeyCode::Char(' ')); // claude: locked, stays ticked
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char(' ')); // codex
    key(&mut app, KeyCode::Enter);
    // Only the choice that was open is stored: claude is not put on the list.
    assert_eq!(
        crate::store::Settings::load().unwrap().shells["zsh"],
        ["codex"]
    );
    assert!(app.derived.shells[zsh].wraps(Harness::Claude));
    app.modal = None;

    // Unticked and saved, it is all gone again.
    app.sel[Page::Settings.index()] = at;
    key(&mut app, KeyCode::Enter);
    assert!(screen(&mut app, 132, 40).contains("[x] claude"));
    key(&mut app, KeyCode::Char(' '));
    key(&mut app, KeyCode::Enter);
    let text = screen(&mut app, 132, 40);
    assert!(text.contains("PATH shims are off"), "{text}");
    assert!(text.contains("out of ~/.zprofile"), "{text}");
    assert!(crate::store::Settings::load().unwrap().shims.is_empty());
    assert!(!crate::shim::dir().exists() && !zprofile.exists());
    // The shell's own list is what counts again.
    key(&mut app, KeyCode::Enter);
    assert!(!app.derived.shells[zsh].wraps(Harness::Claude));
    assert!(app.derived.shells[zsh].wraps(Harness::Codex));
}

#[test]
fn the_shell_tab_lists_shells_and_a_dialog_wraps_agents_in_one() {
    let sb = Sandbox::new();
    sb.demo();
    sb.fake_agents();
    let (mut app, _rx) = app();
    key(&mut app, KeyCode::Char('0'));
    key(&mut app, KeyCode::Char(']'));
    key(&mut app, KeyCode::Char(']'));
    let text = screen(&mut app, 132, 40);
    show("settings/shell", &text);
    for word in [
        "INTEGRATION",
        "SHELL",
        "INTEGRATED",
        "NOT INTEGRATED",
        "zsh",
        "yours",
    ] {
        assert!(text.contains(word), "missing '{word}'\n{text}");
    }
    // The rc file does not load cswap yet, so zsh wraps nothing.
    let zsh = app
        .derived
        .shells
        .iter()
        .position(|s| s.name == "zsh")
        .unwrap();
    assert_eq!(app.derived.shells[zsh].loaded, Some(false));
    assert!(!app.derived.shells[zsh].wraps(Harness::Claude));
    let rc = crate::shell::rc_file("zsh").unwrap();
    assert!(!rc.exists());

    // Enter opens the boxes. Nothing is ticked, and escape changes nothing.
    app.sel[Page::Settings.index()] = zsh;
    key(&mut app, KeyCode::Enter);
    let text = screen(&mut app, 132, 40);
    show("settings/shell dialog", &text);
    assert!(text.contains("Agents wrapped in zsh"), "{text}");
    assert!(text.contains("saving adds one line to ~/.zshrc"), "{text}");
    assert!(
        text.contains("[ ] claude") && text.contains("[ ] codex"),
        "{text}"
    );
    key(&mut app, KeyCode::Char(' '));
    let text = screen(&mut app, 132, 40);
    assert!(
        text.contains("[x] claude") && text.contains("not saved"),
        "{text}"
    );
    key(&mut app, KeyCode::Esc);
    assert!(app.modal.is_none() && !rc.exists());
    assert!(crate::store::Settings::load().unwrap().shells.is_empty());

    // Tick two and save: the list is stored and the line is added once.
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char(' ')); // claude
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char(' ')); // codex
    key(&mut app, KeyCode::Enter);
    assert!(app.modal.is_none(), "{:?}", app.modal);
    let saved = crate::store::Settings::load().unwrap();
    assert_eq!(saved.shells["zsh"], ["claude", "codex"]);
    let text = std::fs::read_to_string(&rc).unwrap();
    assert_eq!(text.matches("cswap shell-init zsh").count(), 1, "{text}");
    let toast = app
        .toast
        .as_ref()
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert!(
        toast.contains("zsh now wraps claude, codex") && toast.contains("~/.zshrc"),
        "{toast}"
    );
    assert!(app.derived.shells[zsh].wraps(Harness::Claude));
    assert!(!app.derived.shells[zsh].wraps(Harness::Grok));

    // Opened again, what is saved is ticked. Unticking one and saving
    // unwraps it, and the rc file is left as it is.
    key(&mut app, KeyCode::Enter);
    let dialog = screen(&mut app, 132, 40);
    assert!(
        dialog.contains("[x] claude") && dialog.contains("[x] codex"),
        "{dialog}"
    );
    assert!(dialog.contains("~/.zshrc loads cswap"), "{dialog}");
    key(&mut app, KeyCode::Char(' ')); // claude off
    key(&mut app, KeyCode::Enter);
    assert_eq!(
        crate::store::Settings::load().unwrap().shells["zsh"],
        ["codex"]
    );
    assert_eq!(std::fs::read_to_string(&rc).unwrap(), text);
    // What `cswap shell-init zsh` prints follows the list.
    let agents = crate::shell::chosen(&crate::store::Settings::load().unwrap(), "zsh", |_| true);
    assert_eq!(agents, [Harness::Codex]);
}

#[test]
fn an_account_is_made_the_default_and_wears_a_star_before_its_name() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    app.dispatch(Action::Go(Page::Accounts));
    app.focus = Focus::Main;
    let text = screen(&mut app, 132, 40);
    // Nothing is starred while the root login is the default; the section says so.
    assert!(!text.contains('★'), "{text}");
    assert!(text.contains("default: root"), "{text}");

    // The actions come in groups, and one of them sets the default.
    key(&mut app, KeyCode::Enter);
    let menu = screen(&mut app, 132, 40);
    show("accounts/menu", &menu);
    for word in [
        "LAUNCH",
        "USAGE",
        "ACCOUNT",
        "Set as default",
        "what `cswap claude` starts on",
    ] {
        assert!(menu.contains(word), "missing '{word}'\n{menu}");
    }
    assert!(!menu.contains("Start a bare"), "{menu}");
    let at = |word: &str| menu.find(word).unwrap();
    assert!(at("LAUNCH") < at("Set as default") && at("Set as default") < at("USAGE"));
    assert!(at("ACCOUNT") < at("Rename") && at("Rename") < at("Remove account"));
    // On a screen too short for the titles, every action is still there.
    let short = screen(&mut app, ui::MIN_W, ui::MIN_H);
    assert!(short.contains("Remove account"), "{short}");

    key(&mut app, KeyCode::Char('d'));
    assert!(app.modal.is_none());
    let start = |h| crate::store::HarnessSettings::load(h).unwrap().start;
    assert_eq!(start(Harness::Claude), "1");
    assert_eq!(start(Harness::Codex), "root");
    let text = screen(&mut app, 132, 40);
    show("accounts/default", &text);
    assert!(text.contains("★ work"), "{text}");
    assert_eq!(text.matches("default: root").count(), 1, "{text}");
    assert!(text.contains("★ `cswap claude` starts here."), "{text}");

    // One default per agent: making another the default moves the star.
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char('d'));
    assert_eq!(start(Harness::Claude), "2");
    let text = screen(&mut app, 132, 40);
    assert!(
        text.contains("★ side") && !text.contains("★ work"),
        "{text}"
    );
    // Asking again says so and changes nothing.
    key(&mut app, KeyCode::Char('d'));
    let toast = app
        .toast
        .as_ref()
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert!(toast.contains("already the default"), "{toast}");

    // The star is never in the dot's place but on the Accounts page. The
    // Overview has it in a column of its own; no other page has one.
    for page in Page::ALL {
        if matches!(page, Page::Accounts | Page::Overview | Page::Help) {
            continue;
        }
        app.dispatch(Action::Go(page));
        let text = screen(&mut app, 132, 40);
        assert!(!text.contains('★'), "{page:?} has a star\n{text}");
    }
    app.dispatch(Action::Go(Page::Overview));
    let text = screen(&mut app, 132, 40);
    show("overview/default", &text);
    assert!(text.contains("DEFAULT"), "{text}");
    let starred = |name: &str| {
        text.lines()
            .any(|l| l.contains(&format!("● {name} ")) && l.contains(" ★ "))
    };
    assert!(starred("side") && !starred("work"), "{text}");
    assert_eq!(text.matches('★').count(), 1, "{text}");

    // The root login gets the default back from its own page.
    app.dispatch(Action::Go(Page::Root));
    app.focus = Focus::Main;
    key(&mut app, KeyCode::Char('d'));
    assert_eq!(start(Harness::Claude), "root");
    let text = screen(&mut app, 132, 40);
    show("root/default", &text);
    assert!(
        text.contains("● Claude Code") && text.contains("The default:") && !text.contains('★'),
        "{text}"
    );
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

#[test]
fn the_resets_page_has_three_parts_and_one_weekly_figure_per_account() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    app.dispatch(Action::Go(Page::Resets));
    for (w, h) in [(ui::MIN_W, ui::MIN_H), (132, 40)] {
        let text = screen(&mut app, w, h);
        // No combined list, no type column, and a weekly reset is called
        // that, whatever model limits reset with it.
        for word in ["Coming up", "TYPE", "week +", "Opus", "Fable"] {
            assert!(!text.contains(word), "{w}x{h} says '{word}'\n{text}");
        }
        // A date is a date.
        let lower = text.to_lowercase();
        for word in ["tomorrow", "tmrw", "today"] {
            assert!(!lower.contains(word), "{w}x{h} says '{word}'\n{text}");
        }
        for word in [
            "Spare resets",
            "Weekly resets",
            "Renewals",
            "weekly",
            "USED",
        ] {
            assert!(text.contains(word), "{w}x{h} lacks '{word}'\n{text}");
        }
        // The week's own figure for `work`, on its spare reset and on its
        // weekly reset; the model limits beside it are not shown.
        assert_eq!(text.matches(" 38%").count(), 2, "{text}");
    }
}

#[test]
fn enter_or_a_second_click_fades_every_other_accounts_resets() {
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    app.dispatch(Action::Go(Page::Resets));
    app.focus = Focus::Main;
    let faint = app.theme.faint();
    // For each row of an account, top to bottom: where it is on the screen,
    // whether it has a marker in front, and whether its name is faded.
    let rows = |app: &mut App, name: &str| {
        let mut term = Terminal::new(TestBackend::new(132, 40)).unwrap();
        term.draw(|f| ui::draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut out = Vec::new();
        for y in 0..40 {
            let row: String = (0..132).map(|x| buf[(x, y)].symbol()).collect();
            if let Some(at) = row.find(&format!("● {name}")) {
                let x = row[..at].chars().count() as u16;
                let marked = buf[(x - 1, y)].symbol() == "▌";
                out.push((y, marked, buf[(x + 2, y)].fg == faint));
            }
        }
        out
    };
    let look = |app: &mut App, name: &str| -> Vec<(bool, bool)> {
        rows(app, name)
            .into_iter()
            .map(|(_, m, f)| (m, f))
            .collect()
    };
    const PLAIN: (bool, bool) = (false, false);
    const SELECTED: (bool, bool) = (true, false);
    const FADED: (bool, bool) = (false, true);

    // The rows, top to bottom: work's spare reset, the weekly resets of work
    // and side, then the renewals of side and work. Selecting one marks that
    // row and nothing else.
    app.sel[Page::Resets.index()] = 0;
    assert_eq!(look(&mut app, "work"), [SELECTED, PLAIN, PLAIN]);
    assert_eq!(look(&mut app, "side"), [PLAIN, PLAIN]);
    assert!(screen(&mut app, 132, 40).contains("⏎ only this account"));

    // Enter narrows the page to the row's account: the others fade.
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.spot.as_deref(), Some("claude-1"));
    assert_eq!(look(&mut app, "work"), [SELECTED, PLAIN, PLAIN]);
    assert_eq!(look(&mut app, "side"), [FADED, FADED]);
    let text = screen(&mut app, 132, 40);
    assert!(
        text.contains("⏎ show all") && text.contains("esc show all"),
        "{text}"
    );

    // The selection still moves anywhere; a faded row keeps its marker.
    app.sel[Page::Resets.index()] = 2;
    assert_eq!(look(&mut app, "side"), [(true, true), FADED]);
    // Enter there narrows to that account instead.
    key(&mut app, KeyCode::Enter);
    assert_eq!(look(&mut app, "side"), [SELECTED, PLAIN]);
    assert_eq!(look(&mut app, "work"), [FADED, FADED, FADED]);
    // Enter on a row of the account shows them all again.
    key(&mut app, KeyCode::Enter);
    assert!(app.spot.is_none());
    assert_eq!(look(&mut app, "work"), [PLAIN, PLAIN, PLAIN]);

    // Escape shows all first, and only then goes back to the menu.
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Esc);
    assert!(app.spot.is_none() && app.focus == Focus::Main);
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.focus, Focus::Nav);

    // One click selects a row; a second click on it narrows the page.
    let (y, ..) = rows(&mut app, "work")[1];
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 60,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    app.on_mouse(click);
    assert_eq!((app.row(), app.spot.is_none()), (1, true));
    assert_eq!(look(&mut app, "side"), [PLAIN, PLAIN]);
    app.on_mouse(click);
    assert_eq!(look(&mut app, "work"), [PLAIN, SELECTED, PLAIN]);
    assert_eq!(look(&mut app, "side"), [FADED, FADED]);

    // Leaving the page shows everything again.
    app.dispatch(Action::Go(Page::Overview));
    app.dispatch(Action::Go(Page::Resets));
    assert!(app.spot.is_none());
}

#[test]
fn the_usage_page_boxes_accounts_by_agent_and_lists_every_limit() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    app.dispatch(Action::Go(Page::Usage));
    for (w, h) in [(ui::MIN_W, ui::MIN_H), (132, 40)] {
        let text = screen(&mut app, w, h);
        let at = |word: &str| {
            text.find(word)
                .unwrap_or_else(|| panic!("no '{word}' at {w}x{h}\n{text}"))
        };
        // One box per agent, each holding its own accounts.
        assert!(at("╭ Claude Code") < at("● work") && at("● side") < at("╭ Codex"));
        assert!(at("╭ Codex") < at("● team"));
        // The plan sits under the name, beside the second limit.
        let rows: Vec<&str> = text.lines().collect();
        let work = rows.iter().position(|l| l.contains("● work")).unwrap();
        assert!(rows[work].contains("week") && rows[work].contains("38%"));
        assert!(rows[work + 1].contains("Ultra") && rows[work + 1].contains("Opus"));
        // The short window is listed on this page, after the weekly limits.
        assert!(rows[work + 2].contains(" 5h ") && rows[work + 2].contains("12%"));
    }
    // A wide screen has the date each limit resets as well.
    let text = screen(&mut app, 132, 40);
    assert!(text.contains("DATE") && text.contains(" · "), "{text}");
    assert!(!screen(&mut app, ui::MIN_W, ui::MIN_H).contains("DATE"));
}

#[test]
fn the_dot_tells_the_agent_so_its_name_is_written_only_without_colour() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    // The Usage page boxes its accounts by agent, so it never needs the tag.
    let pages = [Page::Overview, Page::Resets, Page::Analytics];
    for page in pages.into_iter().chain([Page::Usage]) {
        app.dispatch(Action::Go(page));
        let text = screen(&mut app, 132, 40);
        for tag in ["work claude", "side claude", "team codex"] {
            assert!(!text.contains(tag), "{page:?} says '{tag}'\n{text}");
        }
    }
    // Greyscale has no colour to tell the agents apart, so there it is said.
    crate::store::Settings::update(|s| s.theme = "grey".into()).unwrap();
    app.reload();
    for page in pages {
        app.dispatch(Action::Go(page));
        let text = screen(&mut app, 132, 40);
        assert!(text.contains("work claude"), "{page:?}\n{text}");
    }
}

/// One cell's colour as CSS, or `None` for the terminal's own.
fn css(c: ratatui::style::Color) -> Option<String> {
    match c {
        ratatui::style::Color::Rgb(r, g, b) => Some(format!("rgb({r},{g},{b})")),
        _ => None,
    }
}

/// The screen as HTML, cell by cell, with the colours the terminal would show.
fn html(app: &mut App, w: u16, h: u16) -> String {
    use ratatui::style::Modifier;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    // The padding around the screen takes the screen's own background.
    let bg = css(buf[(0, 0)].bg).unwrap_or_else(|| "#13161e".to_string());
    let mut out = format!("<pre style=\"background:{bg}\">");
    for y in 0..h {
        for x in 0..w {
            let cell = &buf[(x, y)];
            let mut style = String::new();
            if let Some(c) = css(cell.fg) {
                style.push_str(&format!("color:{c};"));
            }
            if let Some(c) = css(cell.bg) {
                style.push_str(&format!("background:{c};"));
            }
            if cell.modifier.contains(Modifier::BOLD) {
                style.push_str("font-weight:600;");
            }
            let text = cell
                .symbol()
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            out.push_str(&format!("<span style=\"{style}\">{text}</span>"));
        }
        out.push('\n');
    }
    out.push_str("</pre>");
    out
}

/// `COLORTERM=truecolor cargo test readme_shots -- --ignored` writes the
/// pages the README shows into `target/shots/` as HTML, with made-up
/// accounts. `scripts/shots.sh` turns them into the pictures.
#[test]
#[ignore = "writes the README's pictures"]
fn readme_shots() {
    let sb = Sandbox::new();
    sb.demo();
    std::env::remove_var("HERDR_APP_HOME");
    std::fs::create_dir_all(crate::paths::home().join(".config/herdr-app")).unwrap();
    let (mut app, _rx) = app();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/shots");
    std::fs::create_dir_all(&dir).unwrap();
    for (page, name) in [
        (Page::Overview, "overview"),
        (Page::Usage, "usage"),
        (Page::Accounts, "accounts"),
        (Page::Resets, "resets"),
        (Page::Integrations, "integrations"),
    ] {
        app.dispatch(Action::Go(page));
        app.focus = Focus::Main;
        let page = format!(
            "<!doctype html><meta charset=\"utf-8\"><style>\
             body{{margin:0;background:#ff00ff;}}\
             pre{{display:inline-block;margin:0;padding:18px 20px;color:#d6d8e0;\
             font:15px/normal 'DejaVu Sans Mono',monospace;}}</style>{}",
            html(&mut app, 118, 30)
        );
        std::fs::write(dir.join(format!("{name}.html")), page).unwrap();
    }
}

#[test]
fn the_help_page_scrolls_to_its_end_and_no_further() {
    let sb = Sandbox::new();
    sb.demo();
    let (mut app, _rx) = app();
    key(&mut app, KeyCode::Char('?'));
    let top = screen(&mut app, ui::MIN_W, ui::MIN_H);
    show("help/top", &top);
    assert!(top.contains("STARTING AN AGENT"), "{top}");
    assert!(!top.contains("KEYS · SETTINGS PAGE"), "{top}");
    // Far more key presses than there are lines: the last line is on screen,
    // and one step up moves the text at once.
    for _ in 0..500 {
        key(&mut app, KeyCode::Down);
    }
    let end = screen(&mut app, ui::MIN_W, ui::MIN_H);
    show("help/end", &end);
    assert!(end.contains("save them all"), "{end}");
    key(&mut app, KeyCode::Up);
    assert_ne!(screen(&mut app, ui::MIN_W, ui::MIN_H), end);
    // The page tells every flag the command line takes.
    app.sel[Page::Help.index()] = 0;
    let text = screen(&mut app, 132, 60);
    for (flag, what) in crate::wrap::FLAGS {
        assert!(text.contains(flag) && text.contains(what), "{flag}\n{text}");
    }
}

/// The Integrations page installs the herdr layout from the files of this
/// repository, on a thread, and says where it stands before and after.
#[test]
fn the_herdr_layout_installs_from_the_integrations_page() {
    let sb = Sandbox::new();
    sb.demo();
    std::env::set_var(
        "CSWAP_INTEGRATIONS",
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("integrations"),
    );
    std::env::remove_var("HERDR_APP_HOME");
    std::env::set_var("CSWAP_HERDR_APP", "/nonexistent/herdr-app");
    let (mut app, rx) = app();

    // No herdr: nothing to install into, and the menu says so.
    key(&mut app, KeyCode::Char('i'));
    let text = screen(&mut app, 132, 40);
    assert!(text.contains("herdr is not on this machine"), "{text}");
    key(&mut app, KeyCode::Enter);
    let text = screen(&mut app, 132, 40);
    assert!(text.contains("Install"), "{text}");
    key(&mut app, KeyCode::Enter);
    assert!(!app.installing, "an install started without herdr");
    key(&mut app, KeyCode::Esc);

    // With herdr's folder there: not installed, then installed.
    let home = crate::paths::home().join(".config").join("herdr-app");
    std::fs::create_dir_all(&home).unwrap();
    app.dispatch(Action::Go(Page::Integrations));
    let text = screen(&mut app, 132, 40);
    assert!(text.contains("not installed"), "{text}");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    assert!(app.installing);
    assert!(screen(&mut app, 132, 40).contains("installing"));
    let msg = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the install never answered");
    app.on_msg(msg);
    assert!(!app.installing);
    let text = screen(&mut app, 132, 40);
    assert!(text.contains("installed 1."), "{text}");
    assert!(text.contains("files written"), "{text}");
    assert!(home.join("layouts/cswap.json").is_file());
    assert!(home.join("mods/cswap-chrome/main.js").is_file());

    // Showing it needs herdr's own tool; without it the page says what to do.
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('s'));
    let text = screen(&mut app, 132, 40);
    assert!(text.contains("Settings → Layout"), "{text}");

    std::env::remove_var("CSWAP_INTEGRATIONS");
    std::env::remove_var("CSWAP_HERDR_APP");
}
