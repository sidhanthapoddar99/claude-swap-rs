//! One module per page, plus the pieces several pages draw the same way.

use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::app::App;
use super::theme::{Theme, DOT, RING, STAR};
use super::widgets::{self, fit};
use crate::model::{self, Window};
use crate::store::Account;

pub mod accounts;
pub mod analytics;
pub mod health;
pub mod help;
pub mod overview;
pub mod resets;
pub mod root;
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

/// `● ` in the agent's colour, in front of an account's name. The default
/// account of its agent wears a star there instead.
pub fn dot(app: &App, acc: &Account) -> Span<'static> {
    let glyph = if app.is_default(acc) { STAR } else { DOT };
    Span::styled(
        format!("{glyph} "),
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

/// A date made to fit a column of `w` cells. It stays a date: the separator
/// dot goes first, then the weekday.
pub fn fit_date(text: String, w: usize) -> String {
    if widgets::width(&text) <= w {
        return text;
    }
    let text = text.replace(" · ", " ");
    if widgets::width(&text) <= w {
        return text;
    }
    // `Fri 13 Nov 12:05 pm` -> `13 Nov 12:05 pm`
    match text.split_once(' ') {
        Some((_, rest)) => rest.to_string(),
        None => text,
    }
}

/// How pressing a spare reset's expiry is: one about to run out stands out.
pub fn expiry_color(
    t: &Theme,
    at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> ratatui::style::Color {
    match at {
        Some(at) if at - now <= chrono::Duration::hours(24) => t.bad(),
        Some(at) if at - now <= chrono::Duration::days(3) => t.warn(),
        _ => t.text(),
    }
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

/// The width of a limit's name: `week`, `Fable`, `5h`, `extra`.
pub const LIMIT_W: usize = 7;
/// The narrowest a limit's bar gets before the date gives way to it.
pub const BAR_MIN: usize = 8;

/// The widths of one limit's line: its bar, and the date the limit resets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitCols {
    /// Zero leaves the bar out, on a line too narrow for one.
    pub bar: usize,
    /// Zero leaves the date out.
    pub date: usize,
}

impl LimitCols {
    /// Fit a limit's line into `w` cells, with a bar of `bar_max` cells at
    /// the most. The date goes when there is no room for it and the bar.
    pub fn new(w: usize, bar_max: usize, clock_24h: bool) -> LimitCols {
        let rest = w.saturating_sub(LIMIT_W + 1 + window_w(0));
        // `Fri 13 Nov · 13:05`, or `Fri 13 Nov · 12:05 pm`; sixteen cells
        // hold either once `fit_date` has shortened it.
        let full = if clock_24h { 18 } else { 21 };
        let date = [full, 16]
            .into_iter()
            .find(|d| rest > BAR_MIN + d)
            .unwrap_or(0);
        let taken = if date > 0 { date + 1 } else { 0 };
        LimitCols {
            bar: rest.saturating_sub(taken).clamp(BAR_MIN, bar_max),
            date,
        }
    }

    /// Everything from the bar to the end of the line.
    pub fn cells_w(&self) -> usize {
        let bar = if self.bar > 0 {
            window_w(self.bar)
        } else {
            4 + 2 + IN_W
        };
        bar + if self.date > 0 { 1 + self.date } else { 0 }
    }

    /// The whole line, the limit's name included.
    pub fn width(&self) -> usize {
        LIMIT_W + 1 + self.cells_w()
    }

    /// The titles of the columns, as wide as the lines under them.
    pub fn header(&self, t: &Theme) -> Vec<Span<'static>> {
        let mut out = vec![head(t, "LIMIT", LIMIT_W + 1)];
        if self.bar > 0 {
            out.push(head(t, "USED", self.bar + 1 + 4 + 2));
        } else {
            out.push(head(t, "USED", 4 + 2));
        }
        out.push(head(t, "IN", IN_W));
        if self.date > 0 {
            out.push(Span::raw(" "));
            out.push(head(t, "DATE", self.date));
        }
        out
    }
}

/// Every limit of an account, a line each: its name, the bar, the percent
/// used, how long until it resets and the date. A warning goes under the
/// limit it is about; pay-as-you-go credit and a failed refresh come last.
/// An account with nothing to show gets one line saying why.
pub fn limit_lines(
    app: &App,
    acc: &Account,
    c: &LimitCols,
    now: DateTime<Utc>,
) -> Vec<Vec<Span<'static>>> {
    let t = &app.theme;
    let line_w = c.width();
    let note_w = c.cells_w();
    let signed_in = app.derived.logged_in.contains(&acc.key());
    let Some(snap) = acc.snapshot().filter(|_| signed_in) else {
        let (text, color) = no_data(app, acc);
        return vec![vec![Span::styled(
            widgets::truncate(&text, line_w),
            Style::default().fg(color),
        )]];
    };
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    for (i, win) in crate::view::all_windows(snap).into_iter().enumerate() {
        let mut cells = vec![
            Span::styled(
                fit(&win.label, LIMIT_W),
                if i == 0 { t.s_text() } else { t.s_dim() },
            ),
            Span::raw(" "),
        ];
        if c.bar > 0 {
            cells.extend(window_cells(app, win, c.bar, now));
        } else {
            cells.push(widgets::pct_span(t, win.used_now(now)));
            cells.push(Span::raw("  "));
            cells.push(Span::styled(fit(&resets_in(win, now), IN_W), t.s_dim()));
        }
        if c.date > 0 {
            let date = match win.resets_at {
                Some(at) if at > now => {
                    fit_date(crate::view::when(at, now, app.settings.clock_24h), c.date)
                }
                _ => "–".to_string(),
            };
            cells.push(Span::raw(" "));
            cells.push(Span::styled(fit(&date, c.date), t.s_dim()));
        }
        rows.push(cells);
        // Said only when the limit will be hit before it resets: the bar and
        // its pace marker already show everything else.
        if let Some(warning) = crate::view::pace_warning(win, now) {
            rows.push(vec![
                Span::raw(" ".repeat(LIMIT_W + 1)),
                Span::styled(widgets::truncate(&warning, note_w), t.s(t.warn())),
            ]);
        }
    }
    if let Some(text) = snap.credits.as_ref().and_then(crate::view::credits_text) {
        rows.push(vec![
            Span::styled(fit("extra", LIMIT_W + 1), t.s_dim()),
            Span::styled(widgets::truncate(&text, note_w), t.s_dim()),
        ]);
    }
    if rows.is_empty() {
        rows.push(vec![Span::styled(
            widgets::truncate("the vendor reported no limits", line_w),
            t.s_dim(),
        )]);
    }
    if let Some(e) = &acc.meta.refresh.error {
        rows.push(vec![
            Span::styled(fit("stale", LIMIT_W + 1), t.s(t.warn())),
            Span::styled(widgets::truncate(e, note_w), t.s(t.warn())),
        ]);
    }
    rows
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

/// The account's name for a list. The star of a default account is not here:
/// it goes in front of the name, where `dot` puts it. `tag` is for lists that
/// mix agents: the dot in front of the name tells them apart by colour, so the
/// agent's command is written after the name only in a theme that has no
/// colour.
pub fn name_cell(app: &App, acc: &Account, w: usize, bold: bool, tag: bool) -> Vec<Span<'static>> {
    let t = &app.theme;
    let bin = acc.harness.spec().bins[0];
    let title = acc.title();
    let tag_w = widgets::width(bin) + 1;
    let tag = tag && !t.agents_by_colour() && w >= widgets::width(&title) + tag_w;
    let tag_used = if tag { tag_w } else { 0 };
    let shown = widgets::truncate(&title, w.saturating_sub(tag_used));
    let mut used = widgets::width(&shown) + tag_used;
    let mut spans = vec![Span::styled(
        shown,
        if bold { t.s_bold() } else { t.s_text() },
    )];
    if tag {
        spans.push(Span::styled(format!(" {bin}"), t.s_faint()));
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
        .roots()
        .any(|a| app.derived.logged_in.contains(&a.key()));
    let key = super::app::Page::Root.hotkey();
    let from_root = format!(
        "Already signed in to an agent? Press {key} for Root, then c to make an account from that login."
    );
    let mut lines = vec!["Press a to add one: name it, then the agent's own sign-in runs."];
    if signed_in {
        lines.push("");
        lines.push(&from_root);
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
