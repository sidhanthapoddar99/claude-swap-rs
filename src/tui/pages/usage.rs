//! Usage: every limit of every account, in one box per agent. The columns are
//! the same in every box, so the bars, the percents, the countdowns and the
//! dates line up down the page.
//!
//! This is the one page that lists the short windows too.

use std::ops::Range;

use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Borders, Paragraph};
use ratatui::Frame;

use super::{
    dot, fit_date, head, name_cell, no_data, paint_row, plan_text, row_style, window_cells,
    window_w, IN_W,
};
use crate::harness::Harness;
use crate::store::Account;
use crate::tui::app::{Action, App, Focus};
use crate::tui::widgets::{self, card_with, fit, truncate, Hits};
use crate::view::{self, all_windows, credits_text, pace_warning};

/// The limit's name: `week`, `Fable`, `5h`, `extra`.
const LABEL_W: usize = 7;
/// The widest the bar gets.
const BAR_MAX: usize = 30;
/// The narrowest it gets before the date gives way to it.
const BAR_MIN: usize = 8;

/// Column widths for a box whose inside is `w` cells wide.
struct Cols {
    name: usize,
    bar: usize,
    /// The date a limit resets. Zero on a screen too narrow for it.
    date: usize,
}

impl Cols {
    fn new(w: usize, clock_24h: bool) -> Cols {
        let name = (w / 5).clamp(14, 22);
        // marker, dot, name, limit, the bar's own cells and the spaces between.
        let fixed = 3 + name + 1 + LABEL_W + 1 + window_w(0);
        let rest = w.saturating_sub(fixed);
        // `Fri 13 Nov · 13:05`, or `Fri 13 Nov · 12:05 pm`; sixteen cells
        // hold either once `fit_date` has shortened it.
        let full = if clock_24h { 18 } else { 21 };
        let date = [full, 16]
            .into_iter()
            .find(|d| rest > BAR_MIN + d)
            .unwrap_or(0);
        let taken = if date > 0 { date + 1 } else { 0 };
        Cols {
            name,
            bar: rest.saturating_sub(taken).clamp(BAR_MIN, BAR_MAX),
            date,
        }
    }

    /// Everything from the bar to the end of the line.
    fn cells_w(&self) -> usize {
        window_w(self.bar) + if self.date > 0 { 1 + self.date } else { 0 }
    }

    /// The whole line.
    fn width(&self) -> usize {
        3 + self.name + 1 + LABEL_W + 1 + self.cells_w()
    }
}

/// The lines of one account. The name is on the first and the plan on the
/// second; beside them one line per limit, with a warning under the limit it
/// is about.
fn block(
    app: &App,
    acc: &Account,
    c: &Cols,
    selected: bool,
    now: DateTime<Utc>,
) -> Vec<Line<'static>> {
    let t = &app.theme;
    let rest_w = LABEL_W + 1 + c.cells_w();
    let note_w = rest_w - LABEL_W - 1;
    // What goes right of the name, a line at a time.
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();

    let signed_in = app.derived.logged_in.contains(&acc.key());
    match acc.snapshot().filter(|_| signed_in) {
        None => {
            let (text, color) = no_data(app, acc);
            rows.push(vec![Span::styled(
                truncate(&text, rest_w),
                Style::default().fg(color),
            )]);
        }
        Some(snap) => {
            for (i, win) in all_windows(snap).into_iter().enumerate() {
                let mut cells = vec![
                    Span::styled(
                        fit(&win.label, LABEL_W),
                        if i == 0 { t.s_text() } else { t.s_dim() },
                    ),
                    Span::raw(" "),
                ];
                cells.extend(window_cells(app, win, c.bar, now));
                if c.date > 0 {
                    let date = match win.resets_at {
                        Some(at) if at > now => {
                            fit_date(view::when(at, now, app.settings.clock_24h), c.date)
                        }
                        _ => "–".to_string(),
                    };
                    cells.push(Span::raw(" "));
                    cells.push(Span::styled(fit(&date, c.date), t.s_dim()));
                }
                rows.push(cells);
                // Said only when the limit will be hit before it resets: the
                // bar and its pace marker already show everything else.
                if let Some(warning) = pace_warning(win, now) {
                    rows.push(vec![
                        Span::raw(" ".repeat(LABEL_W + 1)),
                        Span::styled(truncate(&warning, note_w), t.s(t.warn())),
                    ]);
                }
            }
            if let Some(text) = snap.credits.as_ref().and_then(credits_text) {
                rows.push(vec![
                    Span::styled(fit("extra", LABEL_W + 1), t.s_dim()),
                    Span::styled(truncate(&text, note_w), t.s_dim()),
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
                    Span::styled(fit("stale", LABEL_W + 1), t.s(t.warn())),
                    Span::styled(truncate(e, note_w), t.s(t.warn())),
                ]);
            }
        }
    }

    // The plan has the second line of the name column, so it gets a line
    // even when the account has one limit only.
    let plan = plan_text(app, acc);
    if !plan.is_empty() && rows.len() < 2 {
        rows.push(Vec::new());
    }
    let (mark, _) = row_style(app, selected);
    rows.into_iter()
        .enumerate()
        .map(|(i, cells)| {
            let mut spans = vec![mark.clone()];
            match i {
                0 => {
                    spans.push(dot(app, acc));
                    // The box names the agent, so the name stands alone.
                    spans.extend(name_cell(app, acc, c.name, selected, false));
                }
                1 if !plan.is_empty() => {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(fit(&plan, c.name), t.s_faint()));
                }
                _ => spans.push(Span::raw(" ".repeat(2 + c.name))),
            }
            spans.push(Span::raw(" "));
            spans.extend(cells);
            Line::from(spans)
        })
        .collect()
}

/// One agent's box in the column the page scrolls: the accounts it holds,
/// the line it starts on and how many lines it takes, borders included.
#[derive(Debug, PartialEq)]
struct Group {
    harness: Harness,
    accounts: Range<usize>,
    y: usize,
    h: usize,
}

/// Where everything sits when the whole page is laid out top to bottom.
struct Plan {
    groups: Vec<Group>,
    /// The first line of each account.
    at: Vec<usize>,
    total: usize,
}

/// Lay the accounts out in boxes, one per agent. `lens` is how many lines
/// each account takes, and `gap` the blank lines between two accounts.
fn plan(agents: &[Harness], lens: &[usize], gap: usize) -> Plan {
    let mut groups: Vec<Group> = Vec::new();
    let mut at = Vec::with_capacity(lens.len());
    let mut y = 0;
    for (i, (h, len)) in agents.iter().zip(lens).enumerate() {
        if groups.last().is_some_and(|g| g.harness == *h) {
            y += gap;
        } else {
            // The bottom border of the box before, then the top of this one.
            if let Some(g) = groups.last_mut() {
                y += 1;
                g.h = y - g.y;
            }
            groups.push(Group {
                harness: *h,
                accounts: i..i,
                y,
                h: 0,
            });
            y += 1;
        }
        if let Some(g) = groups.last_mut() {
            g.accounts.end = i + 1;
        }
        at.push(y);
        y += len;
    }
    if let Some(g) = groups.last_mut() {
        y += 1;
        g.h = y - g.y;
    }
    Plan {
        groups,
        at,
        total: y,
    }
}

/// The first line to show so that the selected account is whole on screen,
/// with the border of its box when it is the first or the last one in it.
fn scroll(plan: &Plan, lens: &[usize], sel: usize, view_h: usize) -> usize {
    if plan.total <= view_h {
        return 0;
    }
    let Some(g) = plan.groups.iter().find(|g| g.accounts.contains(&sel)) else {
        return 0;
    };
    let top = plan.at[sel] - usize::from(g.accounts.start == sel);
    let bottom = plan.at[sel] + lens[sel] + usize::from(g.accounts.end == sel + 1);
    bottom.saturating_sub(view_h).min(top)
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let now = Utc::now();
    let accounts = &app.reg.accounts;
    if accounts.is_empty() {
        super::no_accounts(f, area, app);
        return;
    }
    if area.height < 4 || area.width < 12 {
        return;
    }
    let focused = app.focus == Focus::Main;
    // Every box has a border and a cell of padding on each side.
    let c = Cols::new(area.width as usize - 4, app.settings.clock_24h);
    let sel = app.row().min(accounts.len() - 1);
    let blocks: Vec<Vec<Line>> = accounts
        .iter()
        .enumerate()
        .map(|(i, a)| block(app, a, &c, focused && i == sel, now))
        .collect();
    let agents: Vec<Harness> = accounts.iter().map(|a| a.harness).collect();
    let lens: Vec<usize> = blocks.iter().map(Vec::len).collect();

    // One header for all the boxes, above the first.
    let view_h = area.height as usize - 1;
    let mut header = vec![
        Span::raw("   "),
        head(t, "ACCOUNT", c.name + 1),
        head(t, "LIMIT", LABEL_W + 1),
        head(t, "USED", c.bar + 1 + 4 + 2),
        head(t, "IN", IN_W),
    ];
    if c.date > 0 {
        header.push(Span::raw(" "));
        header.push(head(t, "DATE", c.date));
    }
    f.render_widget(
        Paragraph::new(Line::from(header)),
        Rect::new(area.x + 2, area.y, area.width - 4, 1),
    );

    // A blank line between accounts when everything still fits.
    let roomy = plan(&agents, &lens, 1);
    let plan = if roomy.total <= view_h {
        roomy
    } else {
        plan(&agents, &lens, 0)
    };
    let top = scroll(&plan, &lens, sel, view_h);
    let end = top + view_h;
    let screen_y = |line: usize| area.y + 1 + (line - top) as u16;

    for g in &plan.groups {
        let (from, to) = (g.y.max(top), (g.y + g.h).min(end));
        if from >= to {
            continue;
        }
        // A box cut by the edge of the page is drawn without that side.
        let mut sides = Borders::LEFT | Borders::RIGHT;
        if from == g.y {
            sides |= Borders::TOP;
        }
        if to == g.y + g.h {
            sides |= Borders::BOTTOM;
        }
        // The box with the selection wears its agent's colour.
        let color = t.harness(g.harness);
        let border = if focused && g.accounts.contains(&sel) {
            color
        } else {
            t.border()
        };
        let title = Line::from(Span::styled(
            g.harness.name(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        let rect = Rect::new(area.x, screen_y(from), area.width, (to - from) as u16);
        let inner = card_with(f, rect, title, t, border, sides);

        for i in g.accounts.clone() {
            let selected = focused && i == sel;
            for (k, line) in blocks[i].iter().enumerate() {
                let y = plan.at[i] + k;
                if y < top || y >= end {
                    continue;
                }
                let rect = Rect::new(inner.x, screen_y(y), inner.width, 1);
                if selected {
                    paint_row(f, rect, Style::default().bg(t.raised()));
                }
                f.render_widget(Paragraph::new(line.clone()), rect);
                hits.add(rect, Action::Row(i));
            }
        }
    }

    // Say so when accounts are out of view, at the end of the header.
    if plan.total > view_h {
        let whole = |i: &usize| plan.at[*i] >= top && plan.at[*i] + lens[*i] <= end;
        let shown: Vec<usize> = (0..accounts.len()).filter(whole).collect();
        if let (Some(first), Some(last)) = (shown.first(), shown.last()) {
            let text = format!("{}–{} of {}", first + 1, last + 1, accounts.len());
            let tw = widgets::width(&text);
            if c.width() + 2 + tw + 4 <= area.width as usize {
                f.render_widget(
                    Paragraph::new(Span::styled(text, t.s_faint())),
                    Rect::new(area.x + area.width - tw as u16 - 2, area.y, tw as u16, 1),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_columns_never_ask_for_more_than_the_width() {
        // 53 cells is what the smallest frame leaves inside a box.
        for clock_24h in [true, false] {
            for w in 53..200 {
                let c = Cols::new(w, clock_24h);
                assert!(c.width() <= w, "{w}: {}", c.width());
                assert!((BAR_MIN..=BAR_MAX).contains(&c.bar), "{w}: {}", c.bar);
                assert!(c.date == 0 || c.date >= 16, "{w}: {}", c.date);
            }
            // The date is the first thing a narrow screen gives up.
            assert_eq!(Cols::new(53, clock_24h).date, 0);
            assert!(Cols::new(100, clock_24h).date >= 18);
        }
    }

    #[test]
    fn accounts_are_boxed_by_agent_and_the_selected_one_stays_whole() {
        use Harness::{Claude, Codex, Grok};
        let agents = [Claude, Claude, Codex, Grok];
        let lens = [3, 2, 2, 2];
        let p = plan(&agents, &lens, 1);
        // Claude: border, 3 lines, a gap, 2 lines, border.
        assert_eq!(
            p.groups[0],
            Group {
                harness: Claude,
                accounts: 0..2,
                y: 0,
                h: 8
            }
        );
        assert_eq!(p.groups[1].accounts, 2..3);
        assert_eq!((p.groups[1].y, p.groups[1].h), (8, 4));
        assert_eq!((p.groups[2].y, p.groups[2].h), (12, 4));
        assert_eq!(p.at, [1, 5, 9, 13]);
        assert_eq!(p.total, 16);

        // Everything fits: nothing scrolls.
        assert_eq!(scroll(&p, &lens, 3, 16), 0);
        // Ten lines: the first accounts need no scrolling.
        assert_eq!(scroll(&p, &lens, 0, 10), 0);
        assert_eq!(scroll(&p, &lens, 1, 10), 0);
        // The last one is shown to the bottom border of its box.
        assert_eq!(scroll(&p, &lens, 3, 10), 6);
        // Codex is alone in its box, so both of its borders are in view.
        assert_eq!(scroll(&p, &lens, 2, 10), 2);
    }
}
