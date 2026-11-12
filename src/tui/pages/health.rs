//! Health: the checks `cswap doctor` runs, as a list in groups: cswap itself,
//! the shells, the PATH shims, then each agent with its accounts. The
//! selected check is spelled out underneath, with what to do about it.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::{paint_row, row_style, scroll_start, DOT};
use crate::health::{Check, Level};
use crate::tui::app::{Action, App, Focus};
use crate::tui::ui::level_mark;
use crate::tui::widgets::{card, card_note, fit, Hits};

/// One line of the list as it is drawn: the checks, with the titles of their
/// groups between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vis {
    Gap,
    /// The title of the group the check at this index starts.
    Title(usize),
    Row(usize),
}

/// The lines of the list. A blank line sets the groups apart when `gaps`.
fn layout(checks: &[Check], gaps: bool) -> Vec<Vis> {
    let mut out = Vec::new();
    for (i, c) in checks.iter().enumerate() {
        if i == 0 || checks[i - 1].group != c.group {
            if gaps && i > 0 {
                out.push(Vis::Gap);
            }
            out.push(Vis::Title(i));
        }
        out.push(Vis::Row(i));
    }
    out
}

/// What a group's title says at its right: how many of its checks need a
/// look, in the colour of the worst.
fn group_note(checks: &[Check], first: usize) -> Option<(String, Level)> {
    let group = &checks[first].group;
    let mine = || checks[first..].iter().take_while(|c| &c.group == group);
    let count = |l: Level| mine().filter(|c| c.level == l).count();
    match (count(Level::Bad), count(Level::Warn)) {
        (0, 0) => None,
        (0, warn) => Some((format!("{warn} to look at"), Level::Warn)),
        (bad, 0) => Some((format!("{bad} broken"), Level::Bad)),
        (bad, warn) => Some((format!("{bad} broken · {warn} to look at"), Level::Bad)),
    }
}

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
    let h = inner.height as usize;
    let sel = app.row().min(checks.len().saturating_sub(1));
    // The blank lines between groups go first when the list is too long.
    let mut lines = layout(checks, true);
    if lines.len() > h {
        lines = layout(checks, false);
    }
    let sel_line = lines.iter().position(|l| *l == Vis::Row(sel)).unwrap_or(0);
    // The first check of a group brings its title into view with it.
    let anchor = match sel_line.checked_sub(1).map(|n| lines[n]) {
        Some(Vis::Title(_)) if sel_line + 1 < h => sel_line - 1,
        _ => sel_line,
    };
    let start = scroll_start(anchor, lines.len(), h).min(sel_line);
    for (n, line) in lines.iter().enumerate().skip(start).take(h) {
        let rect = Rect::new(inner.x, inner.y + (n - start) as u16, inner.width, 1);
        let i = match *line {
            Vis::Gap => continue,
            Vis::Title(i) => {
                let c = &checks[i];
                let (dot, color) = match c.agent {
                    Some(agent) => (
                        Span::styled(format!(" {DOT} "), t.s(t.harness(agent))),
                        t.dim(),
                    ),
                    None => (Span::raw(" "), t.faint()),
                };
                let title = c.group.to_uppercase();
                let mut spans = vec![
                    dot,
                    Span::styled(
                        title.clone(),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                ];
                if let Some((text, level)) = group_note(checks, i) {
                    let color = if level == Level::Bad {
                        t.bad()
                    } else {
                        t.warn()
                    };
                    spans.push(Span::styled(format!("   {text}"), t.s(color)));
                }
                f.render_widget(Paragraph::new(Line::from(spans)), rect);
                continue;
            }
            Vis::Row(i) => i,
        };
        let c = &checks[i];
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
            Span::raw("  "),
            level_mark(app, c.level),
            Span::raw("  "),
            Span::styled(
                fit(&c.title, w.saturating_sub(6)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn check(group: &str, level: Level) -> Check {
        Check {
            level,
            area: group.to_string(),
            group: group.to_string(),
            agent: None,
            title: String::new(),
            detail: String::new(),
        }
    }

    #[test]
    fn each_group_gets_one_title_and_says_what_needs_a_look() {
        let checks = [
            check("cswap", Level::Ok),
            check("Claude Code", Level::Ok),
            check("Claude Code", Level::Warn),
            check("Claude Code", Level::Info),
            check("Codex", Level::Bad),
            check("Codex", Level::Warn),
        ];
        use Vis::*;
        assert_eq!(
            layout(&checks, true),
            [
                Title(0),
                Row(0),
                Gap,
                Title(1),
                Row(1),
                Row(2),
                Row(3),
                Gap,
                Title(4),
                Row(4),
                Row(5),
            ]
        );
        assert!(!layout(&checks, false).contains(&Gap));
        assert_eq!(group_note(&checks, 0), None);
        assert_eq!(
            group_note(&checks, 1),
            Some(("1 to look at".to_string(), Level::Warn))
        );
        assert_eq!(
            group_note(&checks, 4),
            Some(("1 broken · 1 to look at".to_string(), Level::Bad))
        );
    }
}
