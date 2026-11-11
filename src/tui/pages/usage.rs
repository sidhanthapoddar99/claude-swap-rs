//! Usage: every limit of every account in one grid, so the bars, the percents
//! and the countdowns line up down the page. Nothing here but usage: what
//! resets when is the Resets page, and who an account is the Accounts page.

use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{dot, head, name_cell, no_data, paint_row, row_style, window_cells, window_w};
use crate::store::Account;
use crate::tui::app::{Action, App, Focus};
use crate::tui::widgets::{card, fit, truncate, Hits};
use crate::view::{credits_text, pace_warning, usage_windows};

/// Column widths for an inner width of `w`.
struct Cols {
    name: usize,
    label: usize,
    bar: usize,
    note: usize,
}

fn cols(w: usize) -> Cols {
    let name = (w / 5).clamp(14, 24);
    let label = 9;
    // marker, dot, name, limit, the bar's own cells and the spaces between.
    let fixed = 3 + name + 1 + label + 1 + window_w(0);
    let rest = w.saturating_sub(fixed);
    // The note needs room for `runs out 1d 2h early`; a narrow page gives
    // every cell it has to the bar instead.
    let (bar, note) = if rest >= 8 + 2 + 20 {
        let bar = (rest - 22).clamp(8, 40);
        (bar, rest - bar - 2)
    } else {
        (rest.clamp(8, 40), 0)
    };
    Cols {
        name,
        label,
        bar,
        note,
    }
}

/// The lines of one account: one per limit, the name on the first.
fn block(
    app: &App,
    acc: &Account,
    c: &Cols,
    selected: bool,
    now: DateTime<Utc>,
) -> Vec<Line<'static>> {
    let t = &app.theme;
    let (mark, _) = row_style(app, selected);
    let mut first = vec![mark, dot(app, acc)];
    first.extend(name_cell(app, acc, c.name, selected, true));
    first.push(Span::raw(" "));
    let indent = || Span::raw(" ".repeat(3 + c.name + 1));
    let rest_w = c.label + 1 + window_w(c.bar) + if c.note > 0 { 2 + c.note } else { 0 };

    let signed_in = app.derived.logged_in.contains(&acc.key());
    let Some(snap) = acc.snapshot().filter(|_| signed_in) else {
        let (text, color) = no_data(app, acc);
        first.push(Span::styled(
            truncate(&text, rest_w),
            Style::default().fg(color),
        ));
        return vec![Line::from(first)];
    };

    // Each row after the name: a label, then cells.
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    for (i, win) in usage_windows(snap).into_iter().enumerate() {
        let mut cells = vec![
            Span::styled(
                fit(&win.label, c.label),
                if i == 0 { t.s_text() } else { t.s_dim() },
            ),
            Span::raw(" "),
        ];
        cells.extend(window_cells(app, win, c.bar, now));
        // Said only when the limit will be hit before it resets: the bar and
        // its pace marker already show everything else.
        if let Some(warning) = pace_warning(win, now).filter(|_| c.note > 0) {
            cells.push(Span::raw("  "));
            cells.push(Span::styled(truncate(&warning, c.note), t.s(t.warn())));
        }
        rows.push(cells);
    }
    if let Some(text) = snap.credits.as_ref().and_then(credits_text) {
        rows.push(vec![
            Span::styled(fit("extra", c.label + 1), t.s_dim()),
            Span::styled(truncate(&text, rest_w - c.label - 1), t.s_dim()),
        ]);
    }
    if rows.is_empty() {
        rows.push(vec![Span::styled(
            truncate("the vendor reported no limits", rest_w),
            t.s_dim(),
        )]);
    }
    if let Some(e) = &acc.meta.refresh.error {
        rows.push(vec![
            Span::styled(fit("stale", c.label + 1), t.s(t.warn())),
            Span::styled(truncate(e, rest_w - c.label - 1), t.s(t.warn())),
        ]);
    }

    let mut out = Vec::new();
    for (i, cells) in rows.into_iter().enumerate() {
        let mut spans = if i == 0 {
            std::mem::take(&mut first)
        } else {
            vec![indent()]
        };
        spans.extend(cells);
        out.push(Line::from(spans));
    }
    out
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let now = Utc::now();
    let accounts = &app.reg.accounts;
    if accounts.is_empty() {
        super::no_accounts(f, area, app);
        return;
    }
    let focused = app.focus == Focus::Main;
    let inner = card(f, area, "", t, focused.then(|| t.accent()));
    if inner.height < 3 {
        return;
    }
    let c = cols(inner.width as usize);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("   "),
            head(t, "ACCOUNT", c.name + 1),
            head(t, "LIMIT", c.label + 1),
            head(t, "USED", c.bar + 1 + 4 + 2),
            head(t, "RESETS IN", super::IN_W),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let sel = app.row().min(accounts.len() - 1);
    let blocks: Vec<Vec<Line>> = accounts
        .iter()
        .enumerate()
        .map(|(i, a)| block(app, a, &c, focused && i == sel, now))
        .collect();
    let list_h = inner.height as usize - 1;
    // A blank line between accounts when everything still fits.
    let lines: usize = blocks.iter().map(Vec::len).sum();
    let gap = usize::from(lines + blocks.len() - 1 <= list_h);
    let height = |i: usize| blocks[i].len() + gap;

    // Scroll by whole accounts, so the selected one is never cut.
    let mut start = 0;
    while start < sel && (start..=sel).map(height).sum::<usize>() > list_h + gap {
        start += 1;
    }

    let mut y = inner.y + 1;
    let end = inner.y + inner.height;
    let mut shown_to = start;
    for (i, lines) in blocks.iter().enumerate().skip(start) {
        if y + lines.len() as u16 > end {
            break;
        }
        let selected = focused && i == sel;
        for line in lines {
            let rect = Rect::new(inner.x, y, inner.width, 1);
            if selected {
                paint_row(f, rect, Style::default().bg(t.raised()));
            }
            f.render_widget(Paragraph::new(line.clone()), rect);
            hits.add(rect, Action::Row(i));
            y += 1;
        }
        y += gap as u16;
        shown_to = i + 1;
    }
    if start > 0 || shown_to < accounts.len() {
        super::range_note(
            f,
            area,
            app,
            start + 1,
            shown_to.max(start + 1),
            accounts.len(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_columns_never_ask_for_more_than_the_width() {
        // 53 cells is what the smallest frame leaves a page.
        for w in 53..200 {
            let c = cols(w);
            let note = if c.note > 0 { 2 + c.note } else { 0 };
            let used = 3 + c.name + 1 + c.label + 1 + window_w(c.bar) + note;
            assert!(used <= w, "{w}: {used}");
            assert!(c.note == 0 || c.note >= 20, "{w}: {}", c.note);
            assert!(c.bar >= 8);
        }
    }
}
