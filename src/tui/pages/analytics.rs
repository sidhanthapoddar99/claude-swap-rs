//! Analytics, first version: how fast each account burns through its week and
//! where that lands at the reset. Built from the samples every refresh stores.

use chrono::{DateTime, Duration, Utc};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::{dot, head, main_window, name_cell, no_data, paint_row, row_style, scroll_start};
use crate::history::{self, Sample};
use crate::model;
use crate::tui::app::{Action, App, Focus};
use crate::tui::widgets::{self, card, fit, fit_right, Hits};

/// The weekly percent over the last `days`, one value per cell. A cell with
/// no sample repeats the one before it; cells before the first sample are
/// `None`.
fn series(samples: &[Sample], cells: usize, days: i64, now: DateTime<Utc>) -> Vec<Option<f64>> {
    let start = now - Duration::days(days);
    let step = (days * 86400) as f64 / cells.max(1) as f64;
    let mut out = vec![None; cells];
    for s in samples.iter().filter(|s| s.t >= start) {
        let Some(v) = s.week else { continue };
        let i = (((s.t - start).num_seconds() as f64 / step) as usize).min(cells.saturating_sub(1));
        out[i] = Some(v);
    }
    let mut last = None;
    for v in out.iter_mut() {
        match v {
            Some(x) => last = Some(*x),
            None => *v = last,
        }
    }
    out
}

fn rate(v: Option<f64>) -> String {
    match v {
        Some(r) if r >= 0.05 => format!("{r:.1}%"),
        Some(_) => "0%".to_string(),
        None => "–".to_string(),
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let now = Utc::now();
    let [table, foot] = Layout::vertical([Constraint::Min(5), Constraint::Length(5)]).areas(area);
    let focused = app.focus == Focus::Main;
    let inner = card(f, table, "Burn rate", t, focused.then(|| t.accent()));
    let w = inner.width as usize;
    let name_w = (w / 4).clamp(12, 26);
    let (now_w, day_w, week_w, proj_w) = (6, 9, 9, 10);
    let spark_w = w
        .saturating_sub(3 + name_w + 1 + now_w + day_w + week_w + proj_w + 4)
        .clamp(8, 56);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("   "),
            head(t, "ACCOUNT", name_w + 1),
            head(t, "LAST 7 DAYS", spark_w + 1),
            head(t, &fit_right("NOW", now_w), now_w + 1),
            head(t, &fit_right("PER DAY", day_w), day_w + 1),
            head(t, &fit_right("7D AVG", week_w), week_w + 1),
            head(t, &fit_right("AT RESET", proj_w), proj_w),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let list_h = inner.height.saturating_sub(1) as usize;
    let sel = app.row();
    let start = scroll_start(sel, app.reg.accounts.len(), list_h);
    for (i, acc) in app.reg.accounts.iter().enumerate().skip(start).take(list_h) {
        let rect = Rect::new(inner.x, inner.y + 1 + (i - start) as u16, inner.width, 1);
        let selected = focused && i == sel;
        let (mark, bg) = row_style(app, selected);
        if selected {
            paint_row(f, rect, bg);
        }
        let key = acc.key();
        let mut spans = vec![mark, dot(app, acc)];
        spans.extend(name_cell(app, acc, name_w, selected, true));
        spans.push(Span::raw(" "));
        let signed_in = app.derived.logged_in.contains(&key);
        let empty = Vec::new();
        let samples = app.history.get(&key).unwrap_or(&empty);
        match acc.snapshot().and_then(main_window).filter(|_| signed_in) {
            Some(win) => {
                let used = win.used_now(now);
                let values = series(samples, spark_w, 7, now);
                let known: Vec<f64> = values.iter().flatten().copied().collect();
                let blank = values.len() - known.len();
                spans.push(Span::styled(" ".repeat(blank), Style::default()));
                spans.push(Span::styled(
                    widgets::spark(&known),
                    Style::default().fg(t.harness(acc.harness)),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    fit_right(&format!("{used:.0}%"), now_w),
                    Style::default().fg(t.severity(used)),
                ));
                spans.push(Span::raw(" "));
                let day = history::burn_per_day(samples, 24, now);
                let week = history::burn_per_day(samples, 24 * 7, now);
                spans.push(Span::styled(fit_right(&rate(day), day_w), t.s_text()));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(fit_right(&rate(week), week_w), t.s_dim()));
                spans.push(Span::raw(" "));
                let (text, color) = match win.projected_pct(now) {
                    _ if used >= 99.5 => ("used up".to_string(), t.bad()),
                    Some(p) if p >= 100.0 => ("runs out".to_string(), t.bad()),
                    Some(p) => (format!("{p:.0}%"), t.severity(p)),
                    None => ("–".to_string(), t.faint()),
                };
                spans.push(Span::styled(
                    fit_right(&text, proj_w),
                    Style::default().fg(color),
                ));
            }
            None => {
                let (text, color) = no_data(app, acc);
                spans.push(Span::styled(
                    fit(&text, w.saturating_sub(3 + name_w + 1)),
                    Style::default().fg(color),
                ));
            }
        }
        f.render_widget(Paragraph::new(Line::from(spans)), rect);
        hits.add(rect, Action::Row(i));
    }

    // The selected account in words.
    let inner = card(f, foot, "", t, None);
    let mut lines: Vec<Line> = Vec::new();
    if let Some(acc) = app.reg.accounts.get(sel) {
        let key = acc.key();
        let samples = app.history.get(&key).map(Vec::as_slice).unwrap_or(&[]);
        let win = acc.snapshot().and_then(main_window);
        let text = match (
            win,
            win.and_then(|w| super::pace_text(w, now, app.settings.clock_24h)),
        ) {
            (_, Some((text, _))) => format!("{}: {text}.", acc.title()),
            (Some(w), None) if w.used_now(now) >= 99.5 => format!(
                "{}: the {} limit is used up{}.",
                acc.title(),
                w.label,
                w.resets_at
                    .filter(|at| *at > now)
                    .map(|at| format!(", back in {}", model::span(at - now)))
                    .unwrap_or_default()
            ),
            _ => format!("{}: not enough data for a projection yet.", acc.title()),
        };
        lines.push(Line::from(Span::styled(text, t.s_text())));
        let first = samples.first().map(|s| s.t);
        lines.push(Line::from(Span::styled(
            match first {
                Some(at) => format!(
                    "{} samples since {} · one is stored at every refresh, kept for {} days.",
                    samples.len(),
                    at.with_timezone(&chrono::Local).format("%-d %b"),
                    app.settings.history_days
                ),
                None => "No samples yet. One is stored every time usage is refreshed.".to_string(),
            },
            t.s_dim(),
        )));
    }
    lines.push(Line::from(Span::styled(
        "More is planned here: week-over-week history, per-model split and a cost view.",
        t.s_faint(),
    )));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_series_carries_the_last_value_forward_and_starts_blank() {
        let now = Utc::now();
        let s = |hours_ago: i64, week: f64| Sample {
            t: now - Duration::hours(hours_ago),
            week: Some(week),
            session: None,
            week_reset: None,
        };
        // 7 cells over 7 days: one cell a day.
        let out = series(&[s(100, 10.0), s(30, 40.0)], 7, 7, now);
        assert_eq!(out.len(), 7);
        assert_eq!(out[0], None);
        assert_eq!(out[2], Some(10.0));
        assert_eq!(out[3], Some(10.0));
        assert_eq!(out[5], Some(40.0));
        assert_eq!(out[6], Some(40.0));
    }
}
