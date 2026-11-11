//! Usage: one card per account. The weekly limit gets the bar, a pace marker
//! and the day and time it resets; the short window gets one quiet line.

use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{no_data, pace_text, plan_text, status, status_span, window_cells};
use crate::model::{self, WindowKind};
use crate::store::Account;
use crate::tui::app::{Action, App, Focus};
use crate::tui::theme::DOT;
use crate::tui::widgets::{card, card_note, fit, truncate, Hits};

const LABEL_W: usize = 12;

/// The lines inside one account's card.
fn body(app: &App, acc: &Account, w: usize, now: DateTime<Utc>) -> Vec<Line<'static>> {
    let t = &app.theme;
    let signed_in = app.derived.logged_in.contains(&acc.key());
    let Some(snap) = acc.snapshot().filter(|_| signed_in) else {
        let (text, color) = no_data(app, acc);
        return vec![Line::from(Span::styled(
            truncate(&text, w),
            Style::default().fg(color),
        ))];
    };
    let bar_w = w.saturating_sub(LABEL_W + 5 + 2 + 34).clamp(8, 44);
    let mut out = Vec::new();

    // The long windows, weekly first.
    let mut long: Vec<_> = snap
        .windows
        .iter()
        .filter(|w| w.kind != WindowKind::Session)
        .collect();
    long.sort_by_key(|w| match w.kind {
        WindowKind::Weekly => 0,
        WindowKind::WeeklyScoped => 1,
        _ => 2,
    });
    for (i, win) in long.iter().enumerate() {
        let scoped = win.kind == WindowKind::WeeklyScoped;
        let mut spans = vec![Span::styled(
            fit(&win.label, LABEL_W),
            if scoped { t.s_dim() } else { t.s_text() },
        )];
        spans.extend(window_cells(app, win, bar_w, now));
        out.push(Line::from(spans));
        // Pace is said once, for the window the account is judged by.
        if i == 0 {
            if let Some((text, hot)) = pace_text(win, now, app.settings.clock_24h) {
                out.push(Line::from(vec![
                    Span::raw(" ".repeat(LABEL_W)),
                    Span::styled(
                        truncate(&text, w.saturating_sub(LABEL_W)),
                        if hot { t.s(t.warn()) } else { t.s_faint() },
                    ),
                ]));
            }
        }
    }

    if let Some(s) = snap.session() {
        let used = s.used_now(now);
        let mut text = format!("{used:.0}% used");
        match s.resets_at {
            Some(at) if at > now => {
                text.push_str(&format!(" · resets in {}", model::span(at - now)))
            }
            Some(_) => text = "reset since the last fetch".to_string(),
            None => {}
        }
        out.push(Line::from(vec![
            Span::styled(fit(&format!("{} window", s.label), LABEL_W), t.s_faint()),
            Span::styled(
                text,
                if used >= 85.0 {
                    t.s(t.severity(used))
                } else {
                    t.s_faint()
                },
            ),
        ]));
    }
    if let Some(text) = snap.credits.as_ref().and_then(crate::view::credits_text) {
        out.push(Line::from(vec![
            Span::styled(fit("extra usage", LABEL_W), t.s_faint()),
            Span::styled(text, t.s_dim()),
        ]));
    }
    let grants = acc.grants(now);
    if !grants.is_empty() {
        let n: u32 = grants.iter().map(|g| g.count).sum();
        let soonest = grants.iter().filter_map(|g| g.expires_at).min();
        let mut text = format!("{n} spare reset{}", if n == 1 { "" } else { "s" });
        if let Some(at) = soonest {
            text.push_str(&format!(
                " · the first expires {} (in {})",
                model::landing(at, now, app.settings.clock_24h),
                model::span(at - now)
            ));
        }
        out.push(Line::from(vec![
            Span::styled(fit("resets", LABEL_W), t.s_faint()),
            Span::styled(text, t.s(t.info())),
        ]));
    }
    if let Some(e) = &acc.meta.refresh.error {
        out.push(Line::from(vec![
            Span::styled(fit("last fetch", LABEL_W), t.s_faint()),
            Span::styled(truncate(e, w.saturating_sub(LABEL_W)), t.s(t.warn())),
        ]));
    }
    if out.is_empty() {
        out.push(Line::from(Span::styled(
            "the vendor reported no limits",
            t.s_dim(),
        )));
    }
    out
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let now = Utc::now();
    let accounts = &app.reg.accounts;
    if accounts.is_empty() {
        super::empty(f, area, app, "No accounts yet", &["Press a to add one."]);
        return;
    }
    let w = area.width.saturating_sub(4) as usize;
    let bodies: Vec<Vec<Line>> = accounts.iter().map(|a| body(app, a, w, now)).collect();
    let heights: Vec<u16> = bodies.iter().map(|b| b.len() as u16 + 2).collect();

    // Scroll so the selected card is whole.
    let sel = app.row().min(accounts.len() - 1);
    let mut start = 0;
    while start < sel && heights[start..=sel].iter().sum::<u16>() > area.height {
        start += 1;
    }

    let focused = app.focus == Focus::Main;
    let mut y = area.y;
    for (i, acc) in accounts.iter().enumerate().skip(start) {
        let room = area.y + area.height - y;
        if room < 3 {
            break;
        }
        let h = heights[i].min(room);
        let rect = Rect::new(area.x, y, area.width, h);
        let selected = focused && i == sel;
        let plan = plan_text(app, acc);
        let mut title = format!("{} · {}", acc.title(), acc.harness.name());
        if !plan.is_empty() {
            title.push_str(&format!(" · {plan}"));
        }
        let inner = card(
            f,
            rect,
            &truncate(&title, w.saturating_sub(34)),
            t,
            selected.then(|| t.accent()),
        );
        // The agent's colour, just left of the title.
        f.render_widget(
            Paragraph::new(Span::styled(
                DOT,
                Style::default().fg(t.harness(acc.harness)).bg(t.panel()),
            )),
            Rect::new(rect.x + 1, rect.y, 1, 1),
        );
        let mut note = vec![status_span(t, status(app, acc))];
        if let Some(at) = acc.snapshot().map(|s| s.fetched_at) {
            note.push(Span::styled(
                format!(" · {}", model::ago(at, now)),
                t.s_faint(),
            ));
        }
        card_note(f, rect, Line::from(note), t);
        f.render_widget(Paragraph::new(bodies[i].clone()), inner);
        hits.add(rect, Action::Row(i));
        y += h;
    }
    // Say so when cards are out of view.
    let shown_to = accounts.len().min(
        start
            + heights[start..]
                .iter()
                .scan(0u16, |sum, h| {
                    *sum += *h;
                    (*sum <= area.height).then_some(())
                })
                .count(),
    );
    if start > 0 || shown_to < accounts.len() {
        let text = format!(
            " {}–{} of {} ",
            start + 1,
            shown_to.max(start + 1),
            accounts.len()
        );
        let tw = text.chars().count() as u16;
        if area.width > tw + 2 {
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
}
