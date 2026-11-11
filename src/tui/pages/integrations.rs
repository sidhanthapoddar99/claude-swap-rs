//! Integrations: cswap inside other programs. Each is a set of files in this
//! repository that an install fetches; the page says where each one stands
//! and what it adds.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::{paint_row, row_style};
use crate::integration::{self, State};
use crate::paths;
use crate::tui::app::{Action, App, Focus};
use crate::tui::theme::{DOT, RING};
use crate::tui::widgets::{card, card_note, fit, width, Hits};

/// The state of an integration, as a mark and a few words.
fn state_span(app: &App) -> Span<'static> {
    let t = &app.theme;
    if app.installing {
        return Span::styled("… installing", t.s_dim());
    }
    let s = &app.integration;
    let (glyph, color) = match s {
        State::NoHerdr => (RING, t.faint()),
        State::NotInstalled => (RING, t.dim()),
        State::Installed { .. } if s.wants_install() => (DOT, t.warn()),
        State::Installed { .. } => (DOT, t.ok()),
    };
    Span::styled(format!("{glyph} {}", s.line()), Style::default().fg(color))
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let all = integration::ALL;
    let focused = app.focus == Focus::Main;
    let sel = app.row().min(all.len() - 1);
    let detail_h = all[sel].adds.len() as u16 + 8;
    let [list, detail] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(detail_h)]).areas(area);

    let inner = card(f, list, "Integrations", t, focused.then(|| t.accent()));
    card_note(
        f,
        list,
        Line::from(Span::styled("not part of the binary", t.s_dim())),
        t,
    );
    let w = inner.width as usize;
    for (i, it) in all.iter().enumerate().take(inner.height as usize) {
        let rect = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        let selected = focused && i == sel;
        let (mark, bg) = row_style(app, selected);
        if selected {
            paint_row(f, rect, bg);
        }
        let state = state_span(app);
        let state_w = width(&state.content);
        let name_w = 10;
        let blurb_w = w.saturating_sub(name_w + state_w + 5);
        let line = Line::from(vec![
            mark,
            Span::raw(" "),
            Span::styled(fit(it.name, name_w), t.s_bold()),
            Span::raw(" "),
            Span::styled(fit(it.blurb, blurb_w), t.s_dim()),
            Span::raw(" "),
            state,
        ]);
        f.render_widget(Paragraph::new(line), rect);
        hits.add(rect, Action::Row(i));
    }

    let it = all[sel];
    let inner = card(f, detail, it.name, t, None);
    let mut lines = vec![Line::from(Span::styled("What it adds", t.s_bold()))];
    for add in it.adds {
        lines.push(Line::from(vec![
            Span::styled("  · ", t.s_dim()),
            Span::raw(add.to_string()),
        ]));
    }
    lines.push(Line::default());
    let home = paths::tilde(&integration::herdr_home());
    lines.push(Line::from(Span::styled(
        format!(
            "Goes into {home} as the layout \"{}\" and four mods named cswap-…. Your own layout and mods are left as they are, and a file of yours that is in the way is kept as <file>.mine.",
            integration::LAYOUT
        ),
        t.s_dim(),
    )));
    let next = match &app.integration {
        _ if app.installing => "Fetching the files from GitHub…".to_string(),
        State::NoHerdr => format!("{home} is not there. Start herdr once, then come back."),
        State::NotInstalled => "⏎ installs it: the files are fetched from GitHub once.".to_string(),
        s if s.wants_install() => "⏎ installs this version's files.".to_string(),
        State::Installed { record, .. } => format!(
            "Installed {}. ⏎ shows it in herdr, or installs it again.",
            record
                .installed_at
                .with_timezone(&chrono::Local)
                .format("%-d %b %Y")
        ),
    };
    lines.push(Line::from(Span::styled(next, t.s_dim())));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
