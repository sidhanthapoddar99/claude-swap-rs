//! Resets, in four parts:
//!
//! 1. Coming up: every limit reset and every spare reset that expires, in one
//!    list by time.
//! 2. Spare resets: what each account still holds.
//! 3. Weekly resets: when each account's limits go back to zero.
//! 4. Renewals: when each plan renews or ends.
//!
//! This page only shows. cswap has no way to spend a reset, on purpose.

use chrono::{DateTime, Duration, Utc};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{dot, head, paint_row, row_style, scroll_start};
use crate::tui::app::{Action, App, Focus};
use crate::tui::widgets::{card, card_note, fit, Hits};
use crate::view;

pub use crate::view::{Event as Row, Kind};

/// The four lists, in the order they are drawn.
pub fn sections(app: &App) -> [Vec<Row>; 4] {
    let now = Utc::now();
    [
        view::coming_up(&app.reg, &app.derived.logged_in, now),
        view::grants(&app.reg, now),
        view::limit_resets(&app.reg, &app.derived.logged_in, now),
        view::renewals(&app.reg),
    ]
}

/// Every row of the page, top to bottom. The selection walks this.
pub fn rows(app: &App) -> Vec<Row> {
    sections(app).into_iter().flatten().collect()
}

/// How tall each card is. Each gets what it needs when there is room. When
/// there is not, every card keeps a row and the rest is shared out, the card
/// with the selection first.
fn heights(wants: [u16; 4], avail: u16, first: usize) -> [u16; 4] {
    if wants.iter().sum::<u16>() <= avail {
        return wants;
    }
    let mut out = wants.map(|w| w.min(4));
    let mut left = avail.saturating_sub(out.iter().sum());
    while left > 0 {
        let mut gave = false;
        for step in 0..4 {
            let i = (first + step) % 4;
            if left > 0 && out[i] < wants[i] {
                out[i] += 1;
                left -= 1;
                gave = true;
            }
        }
        if !gave {
            break;
        }
    }
    out
}

/// Column widths, shared by the four tables so they line up.
struct Cols {
    acc: usize,
    tag: usize,
    what: usize,
    in_: usize,
    date: usize,
}

impl Cols {
    fn new(w: usize) -> Cols {
        let wide = w >= 72;
        let acc = (w / 4).clamp(13, 22);
        let tag = 7;
        let in_ = if wide { 9 } else { 8 };
        // Wide enough for `tomorrow · 01:29`; a narrow screen gets a short form.
        let date = if wide { 17 } else { 11 };
        let fixed = 1 + acc + 1 + tag + 1 + in_ + 1 + date + 1;
        // The rest of the line goes to the note.
        let what = w.saturating_sub(fixed).clamp(6, 18);
        Cols {
            acc,
            tag,
            what,
            in_,
            date,
        }
    }

    /// What is left for a note after the five columns.
    fn rest(&self, w: usize) -> usize {
        w.saturating_sub(
            1 + self.acc + 1 + self.tag + 1 + self.what + 1 + self.in_ + 1 + self.date + 1,
        )
    }
}

/// The date for a row. On a narrow screen the separator dot is dropped.
fn date_text(app: &App, row: &Row, now: DateTime<Utc>, c: &Cols) -> String {
    let text = row.date(now, app.settings.clock_24h);
    if c.date < 16 {
        text.replace("tomorrow", "tmrw").replace(" · ", " ")
    } else {
        text
    }
}

/// How pressing a deadline is: a spare reset about to expire stands out.
fn urgency(app: &App, row: &Row, now: DateTime<Utc>) -> Color {
    let t = &app.theme;
    match (row.kind, row.at) {
        (Kind::Spare, Some(at)) if at - now <= Duration::hours(24) => t.bad(),
        (Kind::Spare, Some(at)) if at - now <= Duration::days(3) => t.warn(),
        (Kind::PlanEnds, _) => t.warn(),
        _ => t.text(),
    }
}

fn account_cell(app: &App, key: &str, w: usize) -> Vec<Span<'static>> {
    match app.reg.get(key) {
        Some(acc) => {
            let mut spans = vec![dot(app, acc)];
            spans.extend(super::name_cell(app, acc, w.saturating_sub(2), false, true));
            spans
        }
        None => vec![Span::raw(fit(key, w))],
    }
}

/// One row: account, type, what, countdown, date, then a note in what is left.
fn line(app: &App, row: &Row, now: DateTime<Utc>, c: &Cols, w: usize) -> Vec<Span<'static>> {
    let t = &app.theme;
    let tag_color = match row.kind {
        Kind::Spare => t.info(),
        Kind::Renewal => t.accent(),
        Kind::PlanEnds => t.warn(),
        Kind::Limit => t.dim(),
    };
    let what_color = match row.kind {
        Kind::Spare => t.info(),
        _ => t.text(),
    };
    let (when, date) = match row.at {
        Some(_) => (row.countdown(now), date_text(app, row, now, c)),
        None => ("–".to_string(), "does not expire".to_string()),
    };
    let mut spans = account_cell(app, &row.key, c.acc);
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        fit(row.tag, c.tag),
        Style::default().fg(tag_color),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        fit(&row.what, c.what),
        Style::default().fg(what_color),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        fit(&when, c.in_),
        Style::default().fg(urgency(app, row, now)),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(fit(&date, c.date), t.s_dim()));
    let rest = c.rest(w);
    if rest >= 6 {
        spans.push(Span::raw(" "));
        let style = match (row.kind, row.used) {
            (Kind::Limit, Some(used)) => Style::default().fg(t.severity(used)),
            _ => t.s_faint(),
        };
        spans.push(Span::styled(fit(&row.note, rest - 1), style));
    }
    spans
}

struct Section<'a> {
    title: &'a str,
    /// The header of the third, fourth and fifth columns.
    heads: [&'a str; 3],
    /// Shown when the list is empty.
    empty: &'a str,
    /// Shown on the card's border, at the right.
    note: Option<(String, Color)>,
}

#[allow(clippy::too_many_arguments)]
fn table(
    f: &mut Frame,
    area: Rect,
    app: &App,
    hits: &mut Hits,
    section: &Section,
    rows: &[Row],
    // Where this section's rows start in the page's list, and the selection.
    offset: usize,
    sel: Option<usize>,
) {
    let t = &app.theme;
    let now = Utc::now();
    let inner = card(f, area, section.title, t, None);
    if let Some((text, color)) = &section.note {
        if area.width as usize > section.title.len() + text.len() + 10 {
            card_note(
                f,
                area,
                Line::from(Span::styled(text.clone(), Style::default().fg(*color))),
                t,
            );
        }
    }
    if inner.height == 0 {
        return;
    }
    let w = inner.width as usize;
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(fit(section.empty, w), t.s_dim())),
            inner,
        );
        return;
    }
    let c = Cols::new(w);
    // The header gives way when the card has room for one line only.
    let header = inner.height >= 2;
    if header {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                head(t, "ACCOUNT", c.acc + 1),
                head(t, "TYPE", c.tag + 1),
                head(t, section.heads[0], c.what + 1),
                head(t, section.heads[1], c.in_ + 1),
                head(t, section.heads[2], c.date + 1),
            ])),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }
    let top = inner.y + header as u16;
    let list_h = (inner.height - header as u16) as usize;
    let local = sel
        .and_then(|s| s.checked_sub(offset))
        .filter(|s| *s < rows.len());
    let start = scroll_start(local.unwrap_or(0), rows.len(), list_h);
    for (j, row) in rows.iter().enumerate().skip(start).take(list_h) {
        let rect = Rect::new(inner.x, top + (j - start) as u16, inner.width, 1);
        let selected = local == Some(j);
        let (mark, bg) = row_style(app, selected);
        if selected {
            paint_row(f, rect, bg);
        }
        let mut spans = vec![mark];
        spans.extend(line(app, row, now, &c, w));
        f.render_widget(Paragraph::new(Line::from(spans)), rect);
        hits.add(rect, Action::Row(offset + j));
    }
    // Say so when rows are out of view.
    if rows.len() > list_h && area.width > 16 {
        let text = format!(" {} of {} ", (start + list_h).min(rows.len()), rows.len());
        let tw = text.chars().count() as u16;
        f.render_widget(
            Paragraph::new(Span::styled(text, t.s_faint().bg(t.panel()))),
            Rect::new(
                area.x + area.width - tw - 2,
                area.y + area.height - 1,
                tw,
                1,
            ),
        );
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let now = Utc::now();
    let parts = sections(app);
    let total: usize = parts.iter().map(Vec::len).sum();
    let sel = (app.focus == Focus::Main && total > 0).then(|| app.row().min(total - 1));

    // Which card holds the selection: it gets spare lines first.
    let mut offsets = [0; 4];
    for i in 1..4 {
        offsets[i] = offsets[i - 1] + parts[i - 1].len();
    }
    let first = sel
        .map(|s| (0..4).rev().find(|i| s >= offsets[*i]).unwrap_or(0))
        .unwrap_or(0);

    let wants = [0, 1, 2, 3].map(|i: usize| match parts[i].len() {
        0 => 3,
        n => n as u16 + 3,
    });
    // The view-only line gets a row whenever there is one to spare.
    let noted = wants.iter().sum::<u16>() < area.height;
    let hs = heights(wants, area.height.saturating_sub(noted as u16), first);

    let spare = view::grant_count(&app.reg, now);
    let monthly = parts[2].iter().any(|r| r.tag == "monthly");
    let list = [
        Section {
            title: "Coming up",
            heads: ["RESET", "IN", "DATE"],
            empty: "Nothing yet: reset times appear after the first usage fetch (press r).",
            note: Some((
                "weekly: when it resets · spare: when it expires".to_string(),
                t.faint(),
            )),
        },
        Section {
            title: "Spare resets",
            heads: ["RESET", "EXPIRES", "DATE"],
            empty: "No account holds a spare reset right now.",
            note: (spare > 0).then(|| (format!("{spare} to spend"), t.info())),
        },
        Section {
            title: if monthly {
                "Weekly and monthly resets"
            } else {
                "Weekly resets"
            },
            heads: ["LIMIT", "IN", "DATE"],
            empty: "Nothing yet: reset times appear after the first usage fetch (press r).",
            note: None,
        },
        Section {
            title: "Renewals",
            heads: ["PLAN", "IN", "DATE"],
            empty: "No renewal date is known. Note one on the Accounts page with b.",
            note: None,
        },
    ];

    let mut y = area.y;
    for i in 0..4 {
        let room = (area.y + area.height).saturating_sub(y);
        let h = hs[i].min(room);
        if h < 3 {
            break;
        }
        table(
            f,
            Rect::new(area.x, y, area.width, h),
            app,
            hits,
            &list[i],
            &parts[i],
            offsets[i],
            sel,
        );
        y += h;
        // The view-only line sits right under the spare resets it is about.
        if i == 1 && noted && y < area.y + area.height {
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("  cswap only shows resets. ", t.s_dim()),
                    Span::styled(
                        "Spending one happens in the agent or on the vendor's site, never here.",
                        t.s_faint(),
                    ),
                ])),
                Rect::new(area.x, y, area.width, 1),
            );
            y += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_get_what_they_need_or_share_what_there_is() {
        // Room for all: nothing changes.
        assert_eq!(heights([5, 4, 6, 3], 30, 0), [5, 4, 6, 3]);
        // Too little: every card keeps a row, and the total fits.
        let h = heights([11, 7, 7, 4], 20, 0);
        assert_eq!(h.iter().sum::<u16>(), 20);
        assert!(h.iter().all(|x| *x >= 4), "{h:?}");
        // The card with the selection is served first.
        let h = heights([11, 7, 7, 4], 17, 2);
        assert_eq!(h, [4, 4, 5, 4]);
        // An empty card never grows past its one line.
        assert_eq!(heights([20, 3, 20, 3], 16, 0), [5, 3, 5, 3]);
    }
}
