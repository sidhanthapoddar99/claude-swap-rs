//! Health: the checks `cswap doctor` runs, as a list. The selected one is
//! spelled out underneath, with what to do about it.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::{paint_row, row_style, scroll_start};
use crate::health::Level;
use crate::tui::app::{Action, App, Focus};
use crate::tui::ui::level_mark;
use crate::tui::widgets::{card, card_note, fit, Hits};

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let checks = &app.derived.checks;
    let [list, detail] = Layout::vertical([Constraint::Min(4), Constraint::Length(6)]).areas(area);
    let focused = app.focus == Focus::Main;
    let inner = card(f, list, "Checks", t, focused.then(|| t.accent()));

    let count = |l: Level| checks.iter().filter(|c| c.level == l).count();
    let (bad, warn) = (count(Level::Bad), count(Level::Warn));
    let note = if bad + warn == 0 {
        Span::styled("all clear", t.s(t.ok()))
    } else {
        let mut parts = Vec::new();
        if bad > 0 {
            parts.push(format!("{bad} broken"));
        }
        if warn > 0 {
            parts.push(format!("{warn} to look at"));
        }
        Span::styled(
            parts.join(" · "),
            t.s(if bad > 0 { t.bad() } else { t.warn() }),
        )
    };
    card_note(f, list, Line::from(note), t);

    let w = inner.width as usize;
    let area_w = (w / 4).clamp(10, 18);
    let h = inner.height as usize;
    let sel = app.row().min(checks.len().saturating_sub(1));
    let start = scroll_start(sel, checks.len(), h);
    for (i, c) in checks.iter().enumerate().skip(start).take(h) {
        let rect = Rect::new(inner.x, inner.y + (i - start) as u16, inner.width, 1);
        let selected = focused && i == sel;
        let (mark, bg) = row_style(app, selected);
        if selected {
            paint_row(f, rect, bg);
        }
        let color = match c.level {
            Level::Ok | Level::Info => t.text(),
            Level::Warn => t.warn(),
            Level::Bad => t.bad(),
        };
        let line = Line::from(vec![
            mark,
            Span::raw(" "),
            level_mark(app, c.level),
            Span::raw("  "),
            Span::styled(fit(&c.area, area_w), t.s_dim()),
            Span::raw(" "),
            Span::styled(
                fit(&c.title, w.saturating_sub(area_w + 6)),
                Style::default().fg(color),
            ),
        ]);
        f.render_widget(Paragraph::new(line), rect);
        hits.add(rect, Action::Row(i));
    }

    let inner = card(f, detail, "", t, None);
    if let Some(c) = checks.get(sel) {
        let mut lines = vec![Line::from(vec![
            level_mark(app, c.level),
            Span::raw("  "),
            Span::styled(c.title.clone(), t.s_bold()),
        ])];
        lines.push(Line::from(Span::styled(
            if c.detail.is_empty() {
                "Nothing to do.".to_string()
            } else {
                c.detail.clone()
            },
            t.s_dim(),
        )));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    }
}
