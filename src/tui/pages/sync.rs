//! Sync: what each cswap account shares with the agent's own config dir.
//!
//! An account's config dir is a mirror of links. Only the login and a few
//! per-machine files are real; everything else points at the native dir, so
//! history, settings, skills and plugins are the same on every account.

use chrono::Utc;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{dot, paint_row, row_style, scroll_start};
use crate::model;
use crate::paths;
use crate::store::Account;
use crate::sync::{Report, State};
use crate::tui::app::{Action, App, Focus};
use crate::tui::theme::{CHECK, DOT, RING, WARN};
use crate::tui::widgets::{card, card_note, fit, truncate, width, Hits};

fn summary(app: &App, report: Option<&Report>) -> Span<'static> {
    let t = &app.theme;
    let Some(r) = report else {
        return Span::styled("not checked", t.s_faint());
    };
    let shadowed = r.count(State::Shadowed);
    let pending = r.pending();
    if shadowed > 0 {
        Span::styled(format!("{WARN} {shadowed} shadowed"), t.s(t.warn()))
    } else if pending > 0 {
        Span::styled(format!("{RING} {pending} to link"), t.s(t.info()))
    } else {
        Span::styled(
            format!("{CHECK} {} links", r.count(State::Linked)),
            t.s(t.ok()),
        )
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let keys = app.sync_keys();
    if keys.is_empty() {
        super::empty(
            f,
            area,
            app,
            "Nothing to sync yet",
            &[
                "Each agent's root login uses the agent's own config dir, as it always did.",
                "Add an account (a) and cswap gives it a dir of links to that one,",
                "so settings, history, skills and plugins are shared and only the login differs.",
            ],
        );
        return;
    }
    let list_w = (area.width * 2 / 5).clamp(30, 42);
    let [left, right] =
        Layout::horizontal([Constraint::Length(list_w), Constraint::Min(20)]).areas(area);

    // ---- accounts
    let focused = app.focus == Focus::Main;
    let inner = card(f, left, "", t, focused.then(|| t.accent()));
    let w = inner.width as usize;
    let sel = app.row().min(keys.len() - 1);
    let list_h = inner.height.saturating_sub(2) as usize;
    let start = scroll_start(sel, keys.len(), list_h);
    for (i, key) in keys.iter().enumerate().skip(start).take(list_h) {
        let Some(acc) = app.reg.get(key) else {
            continue;
        };
        let rect = Rect::new(inner.x, inner.y + (i - start) as u16, inner.width, 1);
        let (mark, bg) = row_style(app, i == sel);
        if i == sel {
            paint_row(f, rect, bg);
        }
        let state = summary(app, app.sync_reports.get(key));
        let state_w = 14.min(w / 2);
        let mut spans = vec![mark, dot(app, acc)];
        spans.push(Span::styled(
            fit(&acc.title(), w.saturating_sub(4 + state_w)),
            if i == sel && focused {
                t.s_bold()
            } else {
                t.s_text()
            },
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(fit(&state.content, state_w), state.style));
        f.render_widget(Paragraph::new(Line::from(spans)), rect);
        hits.add(rect, Action::Row(i));
    }
    if inner.height >= 3 {
        f.render_widget(
            Paragraph::new(Span::styled(
                truncate(" the agents' root logins need no sync", w),
                t.s_faint(),
            )),
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
        );
    }

    // ---- the selected account's entries
    let Some(acc) = app.reg.get(&keys[sel]) else {
        return;
    };
    entries(f, right, app, acc);
}

fn entries(f: &mut Frame, area: Rect, app: &App, acc: &Account) {
    let t = &app.theme;
    let now = Utc::now();
    let key = acc.key();
    let native = acc.harness.native_dir();
    let title = format!("{key}  →  {}", paths::tilde(&native));
    // The note on the border gives way to the title when there is no room.
    let noted = area.width as usize >= width(&title) + 30;
    let title_w = (area.width as usize).saturating_sub(if noted { 24 } else { 6 });
    let inner = card(f, area, &truncate(&title, title_w), t, None);
    let hs = app.hs.get(&acc.harness);
    let mut note = Vec::new();
    if let Some(at) = acc.meta.sync.last_synced {
        note.push(Span::styled(
            format!("synced {}", model::ago(at, now)),
            t.s_dim(),
        ));
    } else {
        note.push(Span::styled("never synced", t.s_faint()));
    }
    if noted {
        card_note(f, area, Line::from(note), t);
    }
    let w = inner.width as usize;

    let Some(report) = app.sync_reports.get(&key) else {
        f.render_widget(Paragraph::new(Span::styled("Checking…", t.s_dim())), inner);
        return;
    };

    let mut lines: Vec<Line> = Vec::new();
    let count = |s: State| report.count(s);
    let mut head = vec![
        Span::styled(format!("{} linked", count(State::Linked)), t.s(t.ok())),
        Span::styled(format!("  ·  {} private", count(State::Private)), t.s_dim()),
    ];
    if report.pending() > 0 {
        head.push(Span::styled(
            format!("  ·  {} to link (Enter)", report.pending()),
            t.s(t.info()),
        ));
    }
    if count(State::Shadowed) > 0 {
        head.push(Span::styled(
            format!("  ·  {} shadowed (R repairs)", count(State::Shadowed)),
            t.s(t.warn()),
        ));
    }
    lines.push(Line::from(head));
    if hs.is_some_and(|h| !h.sync.enabled) {
        lines.push(Line::from(Span::styled(
            "Sync at launch is off for this agent (Settings). Links change only when you sync here.",
            t.s(t.warn()),
        )));
    }
    if !native.exists() {
        lines.push(Line::from(Span::styled(
            format!(
                "{} does not exist yet: there is nothing to link to.",
                paths::tilde(&native)
            ),
            t.s(t.warn()),
        )));
    }
    lines.push(Line::default());
    let head_h = lines.len();
    f.render_widget(Paragraph::new(lines), inner);

    // The names, problems first, laid out in columns.
    let order = |s: State| match s {
        State::Shadowed => 0,
        State::Wrong => 1,
        State::Stale => 2,
        State::Missing => 3,
        State::Linked => 4,
        State::Private => 5,
    };
    let mut items: Vec<_> = report.entries.iter().collect();
    items.sort_by(|a, b| {
        order(a.state)
            .cmp(&order(b.state))
            .then(a.name.cmp(&b.name))
    });
    let legend_h = 2;
    let rows = (inner.height as usize).saturating_sub(head_h + legend_h);
    if rows == 0 {
        return;
    }
    let longest = items.iter().map(|e| width(&e.name) + 1).max().unwrap_or(10);
    let col_w = (longest + 4).clamp(18, 34);
    let cols = (w / col_w).max(1);
    let capacity = rows * cols;
    let hidden = items.len().saturating_sub(capacity);
    for (i, e) in items.iter().take(capacity).enumerate() {
        let (col, row) = (i / rows, i % rows);
        // The last cell says how many did not fit.
        if hidden > 0 && i == capacity - 1 {
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!("  + {} more", hidden + 1),
                    t.s_faint(),
                )),
                Rect::new(
                    inner.x + (col * col_w) as u16,
                    inner.y + (head_h + row) as u16,
                    col_w as u16,
                    1,
                ),
            );
            break;
        }
        let (glyph, color) = match e.state {
            State::Linked => (CHECK, t.ok()),
            State::Private => (DOT, t.faint()),
            State::Missing => (RING, t.info()),
            State::Wrong | State::Stale => (RING, t.warn()),
            State::Shadowed => (WARN, t.warn()),
        };
        let name = if e.is_dir {
            format!("{}/", e.name)
        } else {
            e.name.clone()
        };
        let text_style = match e.state {
            State::Linked => t.s_text(),
            State::Private => t.s_faint(),
            _ => Style::default().fg(color),
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{glyph} "), Style::default().fg(color)),
                Span::styled(fit(&name, col_w - 3), text_style),
            ])),
            Rect::new(
                inner.x + (col * col_w) as u16,
                inner.y + (head_h + row) as u16,
                (col_w as u16).min(inner.width.saturating_sub((col * col_w) as u16)),
                1,
            ),
        );
    }

    let legend = Line::from(vec![
        Span::styled(format!("{CHECK} "), t.s(t.ok())),
        Span::styled("shared  ", t.s_faint()),
        Span::styled(format!("{DOT} "), t.s_faint()),
        Span::styled("private  ", t.s_faint()),
        Span::styled(format!("{WARN} "), t.s(t.warn())),
        Span::styled("shadowed", t.s_faint()),
    ]);
    f.render_widget(
        Paragraph::new(legend),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}
