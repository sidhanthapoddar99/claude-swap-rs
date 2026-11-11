//! Resets, in three parts:
//!
//! 1. Spare resets: what each account still holds, and when each expires.
//! 2. Weekly resets: when each account's weekly limit goes back to zero.
//! 3. Renewals: when each plan renews or ends.
//!
//! The three tables share their columns: the account, how many it holds,
//! what resets, how long until then, the date, and how full the account's
//! week is. Only a spare reset has a count.
//!
//! This page only shows. cswap has no way to spend a reset, on purpose.

use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{dot, head, paint_row, row_style, scroll_start};
use crate::tui::app::{Action, App, Focus};
use crate::tui::widgets::{self, card, card_note, fit, Hits};
use crate::view;

pub use crate::view::{Event as Row, Kind};

const PARTS: usize = 3;

/// Said under the spare resets, in two shades.
const VIEW_ONLY: (&str, &str) = (
    "  cswap only shows resets. ",
    "Spending one happens in the agent or on the vendor's site, never here.",
);
const VIEW_ONLY_SHORT: &str = "  View only. Spend a reset in the agent, never here.";

/// The three lists, in the order they are drawn.
pub fn sections(app: &App) -> [Vec<Row>; PARTS] {
    let now = Utc::now();
    [
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
fn heights(wants: [u16; PARTS], avail: u16, first: usize) -> [u16; PARTS] {
    if wants.iter().sum::<u16>() <= avail {
        return wants;
    }
    let mut out = wants.map(|w| w.min(4));
    let mut left = avail.saturating_sub(out.iter().sum());
    while left > 0 {
        let mut gave = false;
        for step in 0..PARTS {
            let i = (first + step) % PARTS;
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

/// The widest the usage bar gets. It is a glance at the week beside the
/// dates, which are what this page is for.
const BAR_MAX: usize = 12;

/// Column widths, shared by the three tables so they line up.
struct Cols {
    acc: usize,
    /// How many spare resets, right-aligned, and the space after it.
    count: usize,
    gap: usize,
    what: usize,
    in_: usize,
    date: usize,
    /// The bar in front of the percent. None on a narrow screen.
    bar: usize,
    /// Whatever is left of the line: the last column.
    last: usize,
}

impl Cols {
    fn new(w: usize, clock_24h: bool) -> Cols {
        let wide = w >= 92;
        let (count, gap) = if wide { (5, 2) } else { (2, 1) };
        let what = if wide { 12 } else { 7 };
        let in_ = if wide { 9 } else { 7 };
        // `Fri 13 Nov · 13:05`, or `Fri 13 Nov · 12:05 pm`. A narrow screen
        // has 16 cells for it, and `date_text` shortens what does not fit.
        let date = match (wide, clock_24h) {
            (true, true) => 18,
            (true, false) => 21,
            (false, _) => 16,
        };
        // The name and the last column share what the others leave. The
        // percent always keeps its four cells.
        let others = 1 + 1 + count + gap + what + 1 + in_ + 1 + date + 1;
        let room = w.saturating_sub(others);
        let acc = (w / 4)
            .clamp(if wide { 14 } else { 11 }, 22)
            .min(room.saturating_sub(4));
        let last = room - acc;
        // A bar joins the percent when there is room for one.
        let bar = if last >= 6 + 1 + 4 {
            (last - 5).min(BAR_MAX)
        } else {
            0
        };
        Cols {
            acc,
            count,
            gap,
            what,
            in_,
            date,
            bar,
            last,
        }
    }
}

/// The date for a row, made to fit its column. It is always a date: when
/// the column is narrow the separator dot goes first, then the weekday.
fn date_text(app: &App, row: &Row, now: DateTime<Utc>, c: &Cols) -> String {
    super::fit_date(row.date(now, app.settings.clock_24h), c.date)
}

/// How pressing a deadline is: a spare reset about to expire stands out.
fn urgency(app: &App, row: &Row, now: DateTime<Utc>) -> Color {
    let t = &app.theme;
    match row.kind {
        Kind::Spare => super::expiry_color(t, row.at, now),
        Kind::PlanEnds => t.warn(),
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

/// The last column: how full the account's week is. For a renewal, where
/// usage says nothing, its cycle.
fn last_cell(app: &App, row: &Row, c: &Cols) -> Vec<Span<'static>> {
    let t = &app.theme;
    if c.last == 0 {
        return Vec::new();
    }
    match (row.kind, row.used) {
        (Kind::Renewal | Kind::PlanEnds, _) => {
            // A narrow screen has room for one short word: that a plan ends
            // is said, and the cycle is left to the Accounts page.
            let fits = widgets::width(&row.note) <= c.last;
            let note = match row.kind {
                _ if fits => row.note.as_str(),
                Kind::PlanEnds => "ends",
                _ => "",
            };
            vec![Span::styled(
                fit(note, c.last),
                if row.kind == Kind::PlanEnds {
                    t.s(t.warn())
                } else {
                    t.s_faint()
                },
            )]
        }
        (_, Some(used)) => {
            let mut spans = Vec::new();
            if c.bar > 0 {
                spans.extend(widgets::bar(t, used, c.bar, row.pace));
                spans.push(Span::raw(" "));
            }
            spans.push(widgets::pct_span(t, used));
            spans
        }
        (_, None) => vec![Span::styled(
            fit(if c.bar > 0 { "no usage yet" } else { "–" }, c.last),
            t.s_faint(),
        )],
    }
}

/// One row: account, count, what resets, countdown, date, usage.
fn line(app: &App, row: &Row, now: DateTime<Utc>, c: &Cols) -> Vec<Span<'static>> {
    let t = &app.theme;
    let what_color = match row.kind {
        Kind::Spare => t.info(),
        _ => t.text(),
    };
    let (when, date) = match row.at {
        Some(_) => (row.countdown(now), date_text(app, row, now, c)),
        None => ("–".to_string(), "does not expire".to_string()),
    };
    // A renewal whose plan nobody has named still gets a cell.
    let what = if row.what.is_empty() {
        "–"
    } else {
        row.what.as_str()
    };
    let count = row.count.map(|n| n.to_string()).unwrap_or_default();
    let mut spans = account_cell(app, &row.key, c.acc);
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        format!("{count:>w$}{}", " ".repeat(c.gap), w = c.count),
        t.s_text(),
    ));
    spans.push(Span::styled(
        fit(what, c.what),
        Style::default().fg(what_color),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        fit(&when, c.in_),
        Style::default().fg(urgency(app, row, now)),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(fit(&date, c.date), t.s_dim()));
    spans.push(Span::raw(" "));
    spans.extend(last_cell(app, row, c));
    spans
}

/// The header over the counts: a word, or a sign when the column is narrow.
fn count_head(section: &Section, c: &Cols) -> &'static str {
    match (section.count, c.count >= 5) {
        (false, _) => "",
        (true, true) => "COUNT",
        (true, false) => "#",
    }
}

struct Section<'a> {
    title: &'a str,
    /// Do the rows have a count? The column keeps its place either way.
    count: bool,
    /// The header of every column after the count.
    heads: [&'a str; 4],
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
    let c = Cols::new(w, app.settings.clock_24h);
    // The header gives way when the card has room for one line only.
    let header = inner.height >= 2;
    if header {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                head(t, "ACCOUNT", c.acc + 1),
                head(
                    t,
                    &format!("{:>w$}", count_head(section, &c), w = c.count),
                    c.count + c.gap,
                ),
                head(t, section.heads[0], c.what + 1),
                head(t, section.heads[1], c.in_ + 1),
                head(t, section.heads[2], c.date + 1),
                head(t, section.heads[3], c.last),
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
        spans.extend(line(app, row, now, &c));
        f.render_widget(Paragraph::new(Line::from(spans)), rect);
        hits.add(rect, Action::Row(offset + j));
    }
    // Say so when rows are out of view.
    if rows.len() > list_h {
        super::range_note(
            f,
            area,
            app,
            start + 1,
            (start + list_h).min(rows.len()),
            rows.len(),
        );
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let now = Utc::now();
    if app.reg.accounts.is_empty() {
        super::no_accounts(f, area, app);
        return;
    }
    let parts = sections(app);
    let total: usize = parts.iter().map(Vec::len).sum();
    let sel = (app.focus == Focus::Main && total > 0).then(|| app.row().min(total - 1));

    // Which card holds the selection: it gets spare lines first.
    let mut offsets = [0; PARTS];
    for i in 1..PARTS {
        offsets[i] = offsets[i - 1] + parts[i - 1].len();
    }
    let first = sel
        .map(|s| (0..PARTS).rev().find(|i| s >= offsets[*i]).unwrap_or(0))
        .unwrap_or(0);

    let wants = [0, 1, 2].map(|i: usize| match parts[i].len() {
        0 => 3,
        n => n as u16 + 3,
    });
    // The view-only line gets a row whenever there is one to spare.
    let noted = wants.iter().sum::<u16>() < area.height;
    let hs = heights(wants, area.height.saturating_sub(noted as u16), first);

    let spare = view::grant_count(&app.reg, now);
    let monthly = parts[1].iter().any(|r| r.what == "monthly");
    let list = [
        Section {
            title: "Spare resets",
            count: true,
            heads: ["RESET", "EXPIRES", "DATE", "USED"],
            empty: "No account holds a spare reset right now.",
            note: (spare > 0).then(|| (format!("{spare} to spend"), t.info())),
        },
        Section {
            title: if monthly {
                "Weekly and monthly resets"
            } else {
                "Weekly resets"
            },
            count: false,
            heads: ["RESET", "IN", "DATE", "USED"],
            empty: "Nothing yet: reset times appear after the first usage fetch (press r).",
            note: None,
        },
        Section {
            title: "Renewals",
            count: false,
            heads: ["PLAN", "IN", "DATE", ""],
            empty: "No renewal date is known. Note one on the Accounts page with b.",
            note: None,
        },
    ];

    let mut y = area.y;
    for i in 0..PARTS {
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
        if i == 0 && noted && y < area.y + area.height {
            let (lead, rest) = VIEW_ONLY;
            // A narrow screen gets the short form, never a cut sentence.
            let line = if widgets::width(lead) + widgets::width(rest) <= area.width as usize {
                Line::from(vec![
                    Span::styled(lead, t.s_dim()),
                    Span::styled(rest, t.s_faint()),
                ])
            } else {
                Line::from(Span::styled(VIEW_ONLY_SHORT, t.s_dim()))
            };
            f.render_widget(Paragraph::new(line), Rect::new(area.x, y, area.width, 1));
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
        assert_eq!(heights([5, 4, 6], 30, 0), [5, 4, 6]);
        // Too little: every card keeps a row, and the total fits.
        let h = heights([11, 7, 7], 16, 0);
        assert_eq!(h.iter().sum::<u16>(), 16);
        assert!(h.iter().all(|x| *x >= 4), "{h:?}");
        // The card with the selection is served first.
        let h = heights([11, 7, 7], 13, 2);
        assert_eq!(h, [4, 4, 5]);
        // An empty card never grows past its one line.
        assert_eq!(heights([20, 3, 20], 13, 0), [5, 3, 5]);
    }

    #[test]
    fn the_columns_fit_and_a_bar_joins_the_percent_when_there_is_room() {
        for clock_24h in [true, false] {
            for w in 53..200 {
                let c = Cols::new(w, clock_24h);
                let used =
                    1 + c.acc + 1 + c.count + c.gap + c.what + 1 + c.in_ + 1 + c.date + 1 + c.last;
                assert!(used <= w, "{w}: {used}");
                // The percent always has its four cells, and the name eleven.
                assert!(c.last >= 4, "{w}: {}", c.last);
                assert!(c.acc >= 11, "{w}: {}", c.acc);
                assert!(c.bar == 0 || c.bar + 5 <= c.last, "{w}");
                assert!(c.bar <= BAR_MAX, "{w}");
            }
            assert_eq!(Cols::new(53, clock_24h).bar, 0);
            assert_eq!(Cols::new(101, clock_24h).bar, BAR_MAX);
        }
    }

    #[test]
    fn a_date_that_does_not_fit_loses_its_dot_then_its_weekday() {
        use crate::tui::pages::fit_date;
        let long = || "Fri 13 Nov · 12:05 pm".to_string();
        assert_eq!(fit_date(long(), 21), "Fri 13 Nov · 12:05 pm");
        assert_eq!(fit_date(long(), 19), "Fri 13 Nov 12:05 pm");
        assert_eq!(fit_date(long(), 16), "13 Nov 12:05 pm");
        assert_eq!(fit_date("Fri 13 Nov".to_string(), 16), "Fri 13 Nov");
    }
}
