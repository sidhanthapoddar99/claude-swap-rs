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
pub mod health;
pub mod overview;
pub mod resets;
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

pub use crate::view::{main_window, pace_text, State as Status};

/// What state an account is in.
pub fn status(app: &App, acc: &Account) -> Status {
    let key = acc.key();
    crate::view::state(
        acc,
        app.derived.logged_in.contains(&key),
        app.live.get(&key).map_or(0, Vec::len),
        app.busy.contains(&key),
        Utc::now(),
    )
}

pub fn status_span(t: &Theme, s: Status) -> Span<'static> {
    let (glyph, color) = match s {
        Status::SignedOut => (RING, t.faint()),
        Status::Fetching => ("…", t.dim()),
        Status::Live(_) => (DOT, t.ok()),
        Status::Blocked => (DOT, t.bad()),
        Status::Stale => (DOT, t.warn()),
        Status::Ready => (DOT, t.dim()),
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

/// `today · 01:29 · in 9h 12m`
pub fn reset_text(app: &App, at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    format!(
        "{} · in {}",
        model::landing(at, now, app.settings.clock_24h),
        model::span(at - now)
    )
}

/// The cells of one usage window: bar, percent and when it resets.
pub fn window_cells(app: &App, w: &Window, bar_w: usize, now: DateTime<Utc>) -> Vec<Span<'static>> {
    let t = &app.theme;
    let used = w.used_now(now);
    let mut spans = widgets::bar(t, used, bar_w, w.elapsed_pct(now));
    spans.push(Span::raw(" "));
    spans.push(widgets::pct_span(t, used));
    spans.push(Span::raw("  "));
    spans.push(match w.resets_at {
        Some(at) if at > now => Span::styled(reset_text(app, at, now), t.s_dim()),
        Some(_) => Span::styled("has reset since the last fetch", t.s(t.ok())),
        None => Span::styled("no reset time reported", t.s_faint()),
    });
    spans
}

/// Why an account has no usage to show, and how loud to say it.
pub fn no_data(app: &App, acc: &Account) -> (String, ratatui::style::Color) {
    let t = &app.theme;
    let key = acc.key();
    let bin = acc.harness.spec().bins[0];
    if !app.derived.logged_in.contains(&key) {
        return if acc.is_native() {
            (format!("not signed in · run `{bin}` to sign in"), t.faint())
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

/// The account's name for a list, with a star when it is its agent's
/// default. `tag` adds the agent's command after the name, for lists that mix
/// agents, where four accounts may all be called `native`.
pub fn name_cell(app: &App, acc: &Account, w: usize, bold: bool, tag: bool) -> Vec<Span<'static>> {
    let t = &app.theme;
    let bin = acc.harness.spec().bins[0];
    let title = acc.title();
    // When the column is narrow the star goes first, then the tag: the tag
    // is what tells four `native` rows apart.
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

/// A name that is clear outside any list: `work`, or `claude native`.
pub fn clear_name(acc: &Account) -> String {
    if acc.is_native() {
        format!("{} native", acc.harness.spec().bins[0])
    } else {
        acc.title()
    }
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
    out.extend(
        lines
            .iter()
            .map(|l| Line::from(Span::styled(l.to_string(), t.s_dim()))),
    );
    let h = out.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect::new(area.x, y, area.width, h.min(area.height));
    f.render_widget(
        ratatui::widgets::Paragraph::new(out).alignment(ratatui::layout::Alignment::Center),
        rect,
    );
}

/// How to launch on this account from a shell.
pub fn launch_hint(app: &App, acc: &Account) -> String {
    crate::view::launch_hint(acc, app.is_default(acc))
}

pub fn help() -> Modal {
    let rows = [
        ("MOVE", ""),
        ("1 – 8", "jump to a page"),
        ("Tab / Shift-Tab", "next / previous page"),
        ("↑ ↓  j k", "move in the menu or the list"),
        (
            "→ Enter",
            "from the menu into the page; open the selected row",
        ),
        ("← Esc", "back to the menu"),
        ("mouse", "click anything; the wheel scrolls"),
        ("", ""),
        ("EVERYWHERE", ""),
        ("r", "refresh usage for every signed-in account"),
        ("a", "add an account"),
        ("?", "this help"),
        ("q", "quit"),
        ("", ""),
        ("ON AN ACCOUNT", ""),
        ("Enter", "all actions for the account"),
        ("o", "open the agent on it (closes the dashboard)"),
        ("d", "make it the default for its agent"),
        ("l", "sign in, or sign in again"),
        ("e  p  b", "alias · plan name · renewal date"),
        ("n", "note a reset the vendor does not report"),
        ("u", "refresh just this account"),
        ("x", "remove"),
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
