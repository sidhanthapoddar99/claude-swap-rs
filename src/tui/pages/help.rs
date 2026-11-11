//! Help: how to start an agent on an account, what cswap's words mean, and
//! every key of the dashboard. It is one column of text that scrolls.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::tui::app::App;
use crate::tui::widgets::{card, kv_wrapped};
use crate::wrap;

/// The width of the left column: a command or a key.
const LEFT_W: usize = 27;

/// One block of the page: a title, and rows of a thing and what it does.
struct Section {
    title: &'static str,
    rows: Vec<(String, String)>,
}

fn section(title: &'static str, rows: &[(&str, &str)]) -> Section {
    Section {
        title,
        rows: rows
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
    }
}

/// Every key of the dashboard, by where it works.
fn keys() -> Vec<Section> {
    vec![
        section(
            "Keys · moving",
            &[
                ("1 – 9, i, 0", "jump to a page"),
                ("?", "this page; again goes back to where you were"),
                ("Tab / Shift-Tab", "next / previous page"),
                ("↑ ↓  j k", "move in the menu or the list"),
                ("PgUp / PgDn", "move eight rows at a time"),
                (
                    "→ Enter",
                    "from the menu into the page; open the selected row",
                ),
                ("←", "back to the menu"),
                (
                    "Esc",
                    "one step back: close a dialog, leave the page, then quit",
                ),
                ("mouse", "click anything; the wheel scrolls"),
            ],
        ),
        section(
            "Keys · everywhere",
            &[
                ("r", "refresh usage for every signed-in account"),
                ("a", "add an account"),
            ],
        ),
        section(
            "Keys · on an account",
            &[
                ("Enter", "all actions for the account, in groups"),
                ("o", "open the agent on it (closes the dashboard)"),
                ("d", "make it the default of its agent"),
                ("l", "sign in, or sign in again"),
                ("e  E", "rename · set aliases"),
                ("p  b", "plan name · renewal date"),
                ("n", "note a reset the vendor does not report"),
                ("u", "refresh just this account"),
                ("x", "remove"),
            ],
        ),
        section(
            "Keys · Root page",
            &[
                ("c", "make an account from the selected login"),
                ("o  d  u", "open · make it the default · refresh"),
            ],
        ),
        section(
            "Keys · Sync page",
            &[
                ("s", "sync every account"),
                ("Enter", "sync the selected account"),
                ("R", "move shadowing files aside and relink"),
            ],
        ),
        section(
            "Keys · Settings page",
            &[
                ("t  T", "next / previous tab; [ and ] do the same"),
                (
                    "← → Enter",
                    "change the selected value, or do what the row says",
                ),
                (
                    "Space, Enter",
                    "in a list of boxes: tick one, save them all",
                ),
            ],
        ),
    ]
}

fn sections() -> Vec<Section> {
    let mut start = vec![
        (
            "claude".to_string(),
            "start on the agent's default. Needs the shell to wrap claude: Settings, Shell tab"
                .to_string(),
        ),
        (
            "cswap claude".to_string(),
            "the same, in any shell, wrapped or not. Every flag below works after either"
                .to_string(),
        ),
    ];
    start.extend(
        wrap::FLAGS
            .iter()
            .map(|(flag, what)| (format!("claude {flag}"), what.to_string())),
    );
    start.extend(
        [
            ("claude --cswap=work", "by name or alias"),
            ("claude --cswap=2", "by its number within the agent"),
            ("claude --cswap=root", "the agent's own login"),
            ("claude --cswap=auto", "the best account right now"),
            ("cswap start", "pick the agent, then the account"),
            (
                "export CSWAP_CLAUDE=work",
                "pin every launch from this terminal to one account",
            ),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string())),
    );
    start.push((
        String::new(),
        "The same goes for codex, agy and grok. Everything else on the line goes to the agent untouched."
            .to_string(),
    ));

    let mut out = vec![
        Section {
            title: "Starting an agent",
            rows: start,
        },
        section(
            "Which account a launch uses",
            &[
                ("1. the flag", "--cswap or --cswap=… on the command line"),
                (
                    "2. the pin",
                    "CSWAP_CLAUDE, CSWAP_CODEX, CSWAP_AGY or CSWAP_GROK in the environment",
                ),
                (
                    "3. the default",
                    "the agent's default, or the best account when auto-pick is on",
                ),
            ],
        ),
        section(
            "Words",
            &[
                (
                    "account",
                    "a login cswap holds for you, under a name you choose. It also answers to its aliases, its number and its email",
                ),
                (
                    "root",
                    "an agent's own login, the one it had before cswap. Each agent has one, on the Root page",
                ),
                (
                    "default",
                    "where a launch that names no account starts: the root login or one account. The Accounts page puts a ★ before its name, the Overview says so in a column. Set it with d",
                ),
                (
                    "auto-pick",
                    "no default: cswap takes the best account at every launch. Settings, Harness tab",
                ),
                (
                    "integrated",
                    "a shell wraps an agent when typing its name there goes through cswap. Settings, Shell tab",
                ),
                (
                    "spare reset",
                    "a reset of the weekly limit you hold and have not spent yet",
                ),
            ],
        ),
    ];
    out.extend(keys());
    out
}

/// The page as lines, wrapped to `w` cells.
fn lines(app: &App, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let room = w.saturating_sub(LEFT_W + 1).max(10);
    let mut out: Vec<Line> = Vec::new();
    for s in sections() {
        if !out.is_empty() {
            out.push(Line::default());
        }
        out.push(Line::from(Span::styled(
            s.title.to_uppercase(),
            Style::default().fg(t.faint()).add_modifier(Modifier::BOLD),
        )));
        for (left, right) in s.rows {
            let mut rows = kv_wrapped(t, &left, LEFT_W, vec![Span::styled(right, t.s_dim())], room);
            // The command or the key stands out; what it does is said dimly.
            if let Some(first) = rows.first_mut().and_then(|l| l.spans.first_mut()) {
                first.style = t.s(t.accent());
            }
            out.extend(rows);
        }
    }
    out
}

/// How many lines the page has when its card is `w` cells wide inside.
pub fn len(app: &App, w: usize) -> usize {
    lines(app, w).len()
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let inner = card(f, area, "", t, None);
    if inner.height == 0 || inner.width < 12 {
        return;
    }
    let all = lines(app, inner.width as usize);
    let view = inner.height as usize;
    let start = app.row().min(all.len().saturating_sub(view));
    let shown: Vec<Line> = all.iter().skip(start).take(view).cloned().collect();
    f.render_widget(Paragraph::new(shown), inner);
    if all.len() > view {
        super::range_note(
            f,
            area,
            app,
            start + 1,
            start + view.min(all.len()),
            all.len(),
        );
    }
}
