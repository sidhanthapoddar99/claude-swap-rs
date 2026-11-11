//! Sessions: the agents running right now, and nothing else. Which account
//! each one is on, since when, and in which folder.

use chrono::Utc;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{head, paint_row, row_style, scroll_start};
use crate::model;
use crate::paths;
use crate::tui::app::{Action, App, Focus, Session};
use crate::tui::theme::DOT;
use crate::tui::widgets::{card, card_note, fit, fit_right, truncate, Hits};

const AGENT_W: usize = 13;
const PID_W: usize = 8;
const SINCE_W: usize = 9;

/// What a system that cannot list other programs' sessions should say.
fn scope_note() -> &'static str {
    if cfg!(target_os = "linux") {
        "An agent started in any terminal shows up here within a few seconds."
    } else {
        "On this system only agents started through cswap are seen."
    }
}

fn row(app: &App, s: &Session, name_w: usize, room: usize, selected: bool) -> Line<'static> {
    let t = &app.theme;
    let (mark, _) = row_style(app, selected);
    let now = Utc::now();
    let acc = app.all.get(&s.key);
    let (agent, color) = match acc {
        Some(a) => (a.harness.name(), t.harness(a.harness)),
        None => ("–", t.faint()),
    };
    let on_default = acc.is_some_and(|a| a.is_native());
    let name = acc.map(|a| a.title()).unwrap_or_else(|| s.key.clone());
    let since = match s.started {
        Some(at) => model::ago(at, now).replace(" ago", ""),
        None => "–".to_string(),
    };
    let folder = match &s.cwd {
        Some(p) => paths::tilde(p),
        None => "–".to_string(),
    };
    Line::from(vec![
        mark,
        Span::styled(format!("{DOT} "), Style::default().fg(color)),
        Span::styled(fit(agent, AGENT_W), t.s_dim()),
        Span::raw(" "),
        Span::styled(
            fit(&name, name_w),
            if on_default {
                t.s_dim()
            } else if selected {
                t.s_bold()
            } else {
                t.s_text()
            },
        ),
        Span::raw(" "),
        Span::styled(fit_right(&s.pid.to_string(), PID_W), t.s_dim()),
        Span::raw("  "),
        Span::styled(fit(&since, SINCE_W), t.s_text()),
        Span::raw(" "),
        Span::styled(truncate(&folder, room), t.s_dim()),
    ])
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    if app.sessions.is_empty() {
        super::empty(f, area, app, "Nothing is running", &[scope_note()]);
        return;
    }
    let focused = app.focus == Focus::Main;
    let inner = card(f, area, "", t, focused.then(|| t.accent()));
    let n = app.sessions.len();
    card_note(
        f,
        area,
        Line::from(Span::styled(format!("{n} running"), t.s(t.ok()))),
        t,
    );
    if inner.height < 2 {
        return;
    }
    let w = inner.width as usize;
    let name_w = (w / 5).clamp(9, 22);
    let fixed = 3 + AGENT_W + 1 + name_w + 1 + PID_W + 2 + SINCE_W + 1;
    let room = w.saturating_sub(fixed);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("   "),
            head(t, "AGENT", AGENT_W + 1),
            head(t, "ACCOUNT", name_w + 1),
            head(t, &fit_right("PID", PID_W), PID_W + 2),
            head(t, "RUNNING", SINCE_W + 1),
            head(t, "FOLDER", room),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let list_h = inner.height as usize - 1;
    let sel = app.row().min(n - 1);
    let start = scroll_start(sel, n, list_h);
    for (i, s) in app.sessions.iter().enumerate().skip(start).take(list_h) {
        let rect = Rect::new(inner.x, inner.y + 1 + (i - start) as u16, inner.width, 1);
        let selected = focused && i == sel;
        if selected {
            paint_row(f, rect, Style::default().bg(t.raised()));
        }
        f.render_widget(Paragraph::new(row(app, s, name_w, room, selected)), rect);
        hits.add(rect, Action::Row(i));
    }
    if n > list_h {
        super::range_note(f, area, app, start + 1, (start + list_h).min(n), n);
    }
}
