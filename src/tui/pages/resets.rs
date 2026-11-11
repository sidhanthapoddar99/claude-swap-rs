//! Resets: the spare resets each account still holds and when they expire,
//! then everything that resets or renews on its own, soonest first.
//!
//! This page only shows. cswap has no way to spend a reset, on purpose.

use chrono::{Duration, Utc};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{dot, head, paint_row, row_style, scroll_start};
use crate::model;
use crate::store::Account;
use crate::tui::app::{Action, App, Focus};
use crate::tui::widgets::{card, card_note, fit, Hits};
use crate::view;

pub use crate::view::{Event as Row, Kind};

/// Every row of the page: spare resets first, then what is coming up.
pub fn rows(app: &App) -> Vec<Row> {
    let now = Utc::now();
    let mut out = view::grants(&app.reg, now);
    out.extend(view::upcoming(
        &app.reg,
        &app.derived.logged_in,
        &app.derived.local,
        now,
    ));
    out
}

fn account_cell(app: &App, acc: Option<&Account>, key: &str, w: usize) -> Vec<Span<'static>> {
    match acc {
        Some(acc) => {
            let mut spans = vec![dot(app, acc)];
            spans.extend(super::name_cell(app, acc, w.saturating_sub(2), false, true));
            spans
        }
        None => vec![Span::raw(fit(key, w))],
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let now = Utc::now();
    let rows = rows(app);
    let n_grants = rows.iter().filter(|r| r.kind == Kind::Grant).count();
    let focused = app.focus == Focus::Main;
    let sel = app.row().min(rows.len().saturating_sub(1));

    let top_h = (n_grants.max(1) as u16 + 3).min(area.height / 2).max(4);
    let [top, note, bottom] = Layout::vertical([
        Constraint::Length(top_h),
        Constraint::Length(1),
        Constraint::Min(4),
    ])
    .areas(area);

    // ---- spare resets
    let inner = card(f, top, "Spare resets", t, None);
    let total = view::grant_count(&app.reg, now);
    if total > 0 {
        card_note(
            f,
            top,
            Line::from(Span::styled(format!("{total} to spend"), t.s(t.info()))),
            t,
        );
    }
    let w = inner.width as usize;
    let acc_w = (w / 4).clamp(14, 26);
    let what_w = 18;
    let when_w = if w >= 96 { 32 } else { 20 };
    if n_grants == 0 {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("No account holds a spare reset right now.", t.s_dim())),
                Line::from(Span::styled(
                    "Claude and Codex report theirs; for the others, press n on an account to note one.",
                    t.s_faint(),
                )),
            ]),
            inner,
        );
    } else {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                head(t, "ACCOUNT", acc_w + 1),
                head(t, "RESET", what_w + 1),
                head(t, "EXPIRES", when_w + 1),
                head(t, "", 0),
            ])),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        let list_h = inner.height.saturating_sub(1) as usize;
        let local_sel = if sel < n_grants { sel } else { 0 };
        let start = scroll_start(local_sel, n_grants, list_h);
        for (i, row) in rows
            .iter()
            .enumerate()
            .take(n_grants)
            .skip(start)
            .take(list_h)
        {
            let rect = Rect::new(inner.x, inner.y + 1 + (i - start) as u16, inner.width, 1);
            let selected = focused && i == sel;
            let (mark, bg) = row_style(app, selected);
            if selected {
                paint_row(f, rect, bg);
            }
            let (when, color) = match row.at {
                None => ("does not expire".to_string(), t.dim()),
                Some(at) => {
                    let left = at - now;
                    let color = if left <= Duration::hours(24) {
                        t.bad()
                    } else if left <= Duration::days(3) {
                        t.warn()
                    } else {
                        t.text()
                    };
                    let text = if w >= 96 {
                        format!(
                            "{} · in {}",
                            model::landing(at, now, app.settings.clock_24h),
                            model::span(left)
                        )
                    } else {
                        format!("in {}", model::span(left))
                    };
                    (text, color)
                }
            };
            let mut spans = vec![mark];
            spans.extend(account_cell(app, app.reg.get(&row.key), &row.key, acc_w));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(fit(&row.what, what_w), t.s(t.info())));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(fit(&when, when_w), Style::default().fg(color)));
            spans.push(Span::raw(" "));
            let room = w.saturating_sub(acc_w + what_w + when_w + 4);
            spans.push(Span::styled(fit(&row.note, room), t.s_faint()));
            f.render_widget(Paragraph::new(Line::from(spans)), rect);
            hits.add(rect, Action::Row(i));
        }
    }

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  cswap only shows resets. ", t.s_dim()),
            Span::styled(
                "Spending one happens in the agent or on the vendor's site, never here.",
                t.s_faint(),
            ),
        ])),
        note,
    );

    // ---- what resets on its own
    let inner = card(f, bottom, "Coming up", t, None);
    let coming = &rows[n_grants..];
    if coming.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "Nothing yet: reset times appear after the first usage fetch (press r).",
                t.s_dim(),
            )),
            inner,
        );
        return;
    }
    let when_w = 19;
    let in_w = 10;
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            head(t, "WHEN", when_w + 1),
            head(t, "IN", in_w + 1),
            head(t, "ACCOUNT", acc_w + 1),
            head(t, "WHAT", 8),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let list_h = inner.height.saturating_sub(1) as usize;
    let local_sel = sel.saturating_sub(n_grants);
    let start = scroll_start(
        if sel >= n_grants { local_sel } else { 0 },
        coming.len(),
        list_h,
    );
    for (j, row) in coming.iter().enumerate().skip(start).take(list_h) {
        let i = n_grants + j;
        let rect = Rect::new(inner.x, inner.y + 1 + (j - start) as u16, inner.width, 1);
        let selected = focused && i == sel;
        let (mark, bg) = row_style(app, selected);
        if selected {
            paint_row(f, rect, bg);
        }
        let when = row.when(now, app.settings.clock_24h);
        let countdown = row.countdown(now);
        let color = match row.kind {
            Kind::PlanEnds | Kind::LoginExpires => t.warn(),
            Kind::Renewal => t.accent(),
            _ => t.text(),
        };
        let mut spans = vec![
            mark,
            Span::styled(fit(&when, when_w), t.s_text()),
            Span::raw(" "),
            Span::styled(fit(&countdown, in_w), t.s_dim()),
            Span::raw(" "),
        ];
        spans.extend(account_cell(app, app.reg.get(&row.key), &row.key, acc_w));
        spans.push(Span::raw(" "));
        let room = w.saturating_sub(when_w + in_w + acc_w + 4);
        let what_w = crate::tui::widgets::width(&row.what).min(room);
        spans.push(Span::styled(
            fit(&row.what, what_w),
            Style::default().fg(color),
        ));
        spans.push(Span::styled(
            fit(&format!("  {}", row.note), room.saturating_sub(what_w)),
            t.s_faint(),
        ));
        f.render_widget(Paragraph::new(Line::from(spans)), rect);
        hits.add(rect, Action::Row(i));
    }
}
