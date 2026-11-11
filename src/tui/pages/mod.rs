//! One module per page, plus the pieces several pages draw the same way.

use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::app::{App, Modal};
use super::theme::{Theme, DOT, RING, STAR};
use super::widgets::{self, fit};
use crate::model::{self, Window};
use crate::store::Account;

pub mod accounts;
pub mod analytics;
pub mod defaults;
pub mod health;
pub mod overview;
pub mod resets;
pub mod sessions;
pub mod settings;
pub mod sync;
pub mod usage;

/// The first row to draw so that `sel` stays inside a list `height` rows tall.
pub fn scroll_start(sel: usize, len: usize, height: usize) -> usize {
    if height == 0 || len <= height {
        return 0;
    }
    // Keep one row of context above and below the selection where possible.
    let start = sel.saturating_sub(height.saturating_sub(2));
    start.min(len - height)
}

pub use crate::view::{main_window, State as Status};

/// What state an account is in.
pub fn status(app: &App, acc: &Account) -> Status {
    let key = acc.key();
    crate::view::state(
        acc,
        app.derived.logged_in.contains(&key),
        app.busy.contains(&key),
        Utc::now(),
    )
}

pub fn status_span(t: &Theme, s: Status) -> Span<'static> {
    let (glyph, color) = match s {
        Status::SignedOut => (RING, t.faint()),
        Status::Fetching => ("…", t.dim()),
        Status::Blocked => (DOT, t.bad()),
        Status::Stale => (DOT, t.warn()),
        Status::Ready => (DOT, t.ok()),
    };
    Span::styled(format!("{glyph} {}", s.label()), Style::default().fg(color))
}

/// `● ` in the agent's colour.
pub fn dot(app: &App, acc: &Account) -> Span<'static> {
    Span::styled(
        format!("{DOT} "),
        Style::default().fg(app.theme.harness(acc.harness)),
    )
}

/// The marker and background of a list row: a bar in the accent colour when
/// selected.
pub fn row_style(app: &App, selected: bool) -> (Span<'static>, Style) {
    let t = &app.theme;
    if selected {
        (
            Span::styled("▌", Style::default().fg(t.accent()).bg(t.raised())),
            Style::default().bg(t.raised()),
        )
    } else {
        (Span::raw(" "), Style::default())
    }
}

/// Paint a whole row's background, so the selection reaches both edges.
pub fn paint_row(f: &mut Frame, rect: Rect, style: Style) {
    f.render_widget(ratatui::widgets::Block::default().style(style), rect);
}

/// The width of the "resets in" cell: `1d 23h`, `23h 59m`, `reset`.
pub const IN_W: usize = 9;

/// How long until a window resets, as short as it can be said.
pub fn resets_in(w: &Window, now: DateTime<Utc>) -> String {
    match w.resets_at {
        Some(at) if at > now => model::until(at, now),
        Some(_) => "reset".to_string(),
        None => "–".to_string(),
    }
}

/// The cells of one usage window, the same on every page so the columns line
/// up: the bar, the percent used, and how long until it resets. The width is
/// always `bar_w + 1 + 4 + 2 + IN_W`.
pub fn window_cells(app: &App, w: &Window, bar_w: usize, now: DateTime<Utc>) -> Vec<Span<'static>> {
    let t = &app.theme;
    let used = w.used_now(now);
    let mut spans = widgets::bar(t, used, bar_w, w.elapsed_pct(now));
    spans.push(Span::raw(" "));
    spans.push(widgets::pct_span(t, used));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(fit(&resets_in(w, now), IN_W), t.s_dim()));
    spans
}

/// The width `window_cells` takes for a bar of `bar_w` cells.
pub const fn window_w(bar_w: usize) -> usize {
    bar_w + 1 + 4 + 2 + IN_W
}

/// Why an account has no usage to show, and how loud to say it.
pub fn no_data(app: &App, acc: &Account) -> (String, ratatui::style::Color) {
    let t = &app.theme;
    let key = acc.key();
    let bin = acc.harness.spec().bins[0];
    if !app.derived.logged_in.contains(&key) {
        return if acc.is_native() {
            (
                format!("not signed in · run `{bin}` and sign in"),
                t.faint(),
            )
        } else {
            ("not signed in · press l".to_string(), t.faint())
        };
    }
    if app.busy.contains(&key) {
        return ("fetching…".to_string(), t.info());
    }
    match &acc.meta.refresh.error {
        Some(e) => (e.clone(), t.warn()),
        None => ("no usage yet · press r".to_string(), t.dim()),
    }
}

/// The account's name for a list, with a star when a bare launch of its agent
/// starts on it. `tag` adds the agent's command after the name, for lists
/// that mix agents.
pub fn name_cell(app: &App, acc: &Account, w: usize, bold: bool, tag: bool) -> Vec<Span<'static>> {
    let t = &app.theme;
    let bin = acc.harness.spec().bins[0];
    let title = acc.title();
    // When the column is narrow the star goes first, then the tag.
    let tag_w = widgets::width(bin) + 1;
    let tag = tag && w >= widgets::width(&title) + tag_w;
    let tag_used = if tag { tag_w } else { 0 };
    let star = app.is_default(acc) && w >= widgets::width(&title) + tag_used + 2;
    let star_w = if star { 2 } else { 0 };
    let shown = widgets::truncate(&title, w.saturating_sub(tag_used + star_w));
    let mut used = widgets::width(&shown) + tag_used + star_w;
    let mut spans = vec![Span::styled(
        shown,
        if bold { t.s_bold() } else { t.s_text() },
    )];
    if tag {
        spans.push(Span::styled(format!(" {bin}"), t.s_faint()));
    }
    if star {
        spans.push(Span::styled(format!(" {STAR}"), t.s(t.accent())));
    }
    used = used.min(w);
    spans.push(Span::raw(" ".repeat(w - used)));
    spans
}

/// The plan as shown in a list.
pub fn plan_text(app: &App, acc: &Account) -> String {
    crate::view::plan_text(acc, &app.derived.local)
}

/// A table header cell.
pub fn head(t: &Theme, text: &str, w: usize) -> Span<'static> {
    Span::styled(
        fit(text, w),
        Style::default().fg(t.faint()).add_modifier(Modifier::BOLD),
    )
}

/// A centred message for a page with nothing to show yet.
pub fn empty(f: &mut Frame, area: Rect, app: &App, title: &str, lines: &[&str]) {
    let t = &app.theme;
    let mut out = vec![Line::from(Span::styled(title.to_string(), t.s_bold()))];
    out.push(Line::default());
    // A line longer than the page is broken at a space, not cut.
    let room = (area.width as usize).saturating_sub(4).max(10);
    for l in lines {
        out.extend(
            widgets::wrap_spans(vec![Span::styled(l.to_string(), t.s_dim())], room)
                .into_iter()
                .map(Line::from),
        );
    }
    let h = out.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect::new(area.x, y, area.width, h.min(area.height));
    f.render_widget(
        ratatui::widgets::Paragraph::new(out).alignment(ratatui::layout::Alignment::Center),
        rect,
    );
}

/// What a page of accounts shows when there are none: how to get the first.
pub fn no_accounts(f: &mut Frame, area: Rect, app: &App) {
    if !app.derived.bins.values().any(|b| b.is_some()) {
        empty(
            f,
            area,
            app,
            "No agent found",
            &[
                "cswap looks for claude, codex, agy and grok on your PATH.",
                "Install one, then open cswap again.",
            ],
        );
        return;
    }
    let signed_in = app
        .all
        .defaults()
        .any(|a| app.derived.logged_in.contains(&a.key()));
    let key = super::app::Page::Defaults.hotkey();
    let from_default = format!(
        "Already signed in to an agent? Press {key} for Defaults, then c to make an account from that login."
    );
    let mut lines = vec!["Press a to add one: name it, then the agent's own sign-in runs."];
    if signed_in {
        lines.push("");
        lines.push(&from_default);
    }
    empty(f, area, app, "No accounts yet", &lines);
}

/// ` 3–7 of 12 ` on the bottom border of a list that does not fit.
pub fn range_note(f: &mut Frame, area: Rect, app: &App, from: usize, to: usize, of: usize) {
    let t = &app.theme;
    let text = format!(" {from}–{to} of {of} ");
    let tw = widgets::width(&text) as u16;
    if area.width > tw + 4 && area.height > 0 {
        f.render_widget(
            ratatui::widgets::Paragraph::new(Span::styled(text, t.s_faint().bg(t.panel()))),
            Rect::new(
                area.x + area.width - tw - 2,
                area.y + area.height - 1,
                tw,
                1,
            ),
        );
    }
}

/// How to launch on this account from a shell.
pub fn launch_hint(app: &App, acc: &Account) -> String {
    crate::view::launch_hint(acc, app.is_default(acc))
}

pub fn help() -> Modal {
    let rows = [
        ("MOVE", ""),
        ("1 – 9, 0", "jump to a page"),
        ("Tab / Shift-Tab", "next / previous page"),
        ("↑ ↓  j k", "move in the menu or the list"),
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
        ("", ""),
        ("EVERYWHERE", ""),
        ("r", "refresh usage for every signed-in account"),
        ("a", "add an account"),
        ("?", "this help"),
        ("", ""),
        ("ON AN ACCOUNT", ""),
        ("Enter", "all actions for the account"),
        ("o", "open the agent on it (closes the dashboard)"),
        ("d", "start a bare launch of its agent here"),
        ("l", "sign in, or sign in again"),
        ("e  E", "rename · set aliases"),
        ("p  b", "plan name · renewal date"),
        ("n", "note a reset the vendor does not report"),
        ("u", "refresh just this account"),
        ("x", "remove"),
        ("", ""),
        ("DEFAULTS PAGE", ""),
        ("c", "make an account from the selected login"),
        ("o  d  u", "open · start bare launches here · refresh"),
        ("", ""),
        ("SYNC PAGE", ""),
        ("s", "sync every account"),
        ("Enter", "sync the selected account"),
        ("R", "move shadowing files aside and relink"),
        ("", ""),
        ("SETTINGS PAGE", ""),
        ("[ ]", "previous / next tab"),
        ("← → Enter", "change the selected value"),
    ];
    Modal::Info {
        title: "Keys".to_string(),
        body: rows
            .iter()
            .map(|(k, v)| {
                if v.is_empty() {
                    k.to_string()
                } else {
                    format!("  {}{v}", fit(k, 18))
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_keeps_the_selection_in_view() {
        assert_eq!(scroll_start(0, 30, 10), 0);
        assert_eq!(scroll_start(5, 30, 10), 0);
        assert_eq!(scroll_start(12, 30, 10), 4);
        assert_eq!(scroll_start(29, 30, 10), 20);
        assert_eq!(scroll_start(3, 4, 10), 0);
    }
}
