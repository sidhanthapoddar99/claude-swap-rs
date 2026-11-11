//! Overview: four numbers, what to use next for each agent, and every account
//! on one line.

use chrono::{DateTime, Utc};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{
    dot, head, launch_hint, main_window, name_cell, no_data, paint_row, plan_text, row_style,
    scroll_start, status, status_span,
};
use crate::harness::Harness;
use crate::health::{self, Level};
use crate::model;
use crate::store::Account;
use crate::tui::app::{Action, App, Focus, Page};
use crate::tui::theme::{DOT, STAR};
use crate::tui::widgets::{self, card, fit, fit_right, width, Hits};

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let now = Utc::now();
    if app.reg.accounts.is_empty() {
        super::empty(
            f,
            area,
            app,
            "No agent found",
            &[
                "cswap looks for claude, codex, agy and grok on your PATH.",
                "Install one, sign in to it once, then open cswap again.",
            ],
        );
        return;
    }
    // One "use next" line per agent that has somewhere to launch.
    let agents: Vec<Harness> = Harness::ALL
        .into_iter()
        .filter(|h| app.reg.of(*h).next().is_some())
        .filter(|h| app.derived.bins.get(h).is_some_and(|b| b.is_some()))
        .collect();
    let next_h = if agents.is_empty() {
        0
    } else {
        agents.len() as u16 + 2
    };
    let [tiles_area, next_area, table_area] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(next_h),
        Constraint::Min(4),
    ])
    .areas(area);

    tiles(f, tiles_area, app, hits, now);
    if !agents.is_empty() {
        use_next(f, next_area, app, hits, &agents);
    }
    table(f, table_area, app, hits, now);
}

/// One number with a caption. `short` is used when the long form does not
/// fit, so a narrow terminal shows `6 / 7` instead of a cut-off sentence.
fn tile(
    f: &mut Frame,
    area: Rect,
    app: &App,
    title: &str,
    value: Span<'static>,
    caption: String,
    short: (&str, &str),
) {
    let t = &app.theme;
    let inner = card(f, area, title, t, None);
    let w = inner.width as usize;
    let fits = width(&value.content) <= w;
    let (text, caption) = if fits {
        (value.content.to_string(), caption)
    } else {
        (short.0.to_string(), short.1.to_string())
    };
    let value = Span::styled(widgets::truncate(&text, w), value.style);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(value),
            Line::from(Span::styled(widgets::truncate(&caption, w), t.s_faint())),
        ]),
        inner,
    );
}

fn tiles(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits, now: DateTime<Utc>) {
    let t = &app.theme;
    let [a, b, c, d] = Layout::horizontal([Constraint::Ratio(1, 4); 4]).areas(area);
    let big = |color: Color| Style::default().fg(color).add_modifier(Modifier::BOLD);

    // Accounts.
    let total = app.reg.accounts.len();
    let signed = app.derived.logged_in.len();
    let agents = Harness::ALL
        .into_iter()
        .filter(|h| {
            app.reg
                .of(*h)
                .any(|x| app.derived.logged_in.contains(&x.key()))
        })
        .count();
    tile(
        f,
        a,
        app,
        "Accounts",
        Span::styled(format!("{signed} signed in"), big(t.text())),
        format!(
            "{total} in all · {agents} agent{}",
            if agents == 1 { "" } else { "s" }
        ),
        (&format!("{signed} / {total}"), "signed in"),
    );
    hits.add(a, Action::Go(Page::Accounts));

    // Running now.
    let sessions: usize = app.live.values().map(Vec::len).sum();
    let names: Vec<String> = app
        .reg
        .accounts
        .iter()
        .filter(|x| app.is_live(&x.key()))
        .map(super::clear_name)
        .collect();
    let (value, caption) = if sessions == 0 {
        (
            Span::styled("nothing", big(t.dim())),
            "no agent is running".to_string(),
        )
    } else {
        (
            Span::styled(
                format!("{sessions} session{}", if sessions == 1 { "" } else { "s" }),
                big(t.ok()),
            ),
            format!("on {}", names.join(", ")),
        )
    };
    tile(
        f,
        b,
        app,
        "Running now",
        value,
        caption,
        (
            &sessions.to_string(),
            if sessions == 1 { "session" } else { "sessions" },
        ),
    );

    // The next weekly reset.
    let soonest = app
        .reg
        .accounts
        .iter()
        .filter(|x| app.derived.logged_in.contains(&x.key()))
        .filter_map(|x| {
            let w = main_window(x.snapshot()?)?;
            Some((w.resets_at.filter(|at| *at > now)?, x, w.used_now(now)))
        })
        .min_by_key(|(at, _, _)| *at);
    let (value, caption, short) = match soonest {
        Some((at, acc, used)) => (
            Span::styled(format!("in {}", model::span(at - now)), big(t.text())),
            format!(
                "{} · {:.0}% left · {}",
                super::clear_name(acc),
                100.0 - used,
                model::landing(at, now, app.settings.clock_24h)
            ),
            (model::span(at - now), acc.title()),
        ),
        None => (
            Span::styled("unknown", big(t.dim())),
            "no usage fetched yet".to_string(),
            ("–".to_string(), "no data".to_string()),
        ),
    };
    tile(
        f,
        c,
        app,
        "Next reset",
        value,
        caption,
        (&short.0, &short.1),
    );
    hits.add(c, Action::Go(Page::Resets));

    // What needs a look.
    let n = health::attention(&app.derived.checks);
    let first = app
        .derived
        .checks
        .iter()
        .filter(|c| c.level >= Level::Warn)
        .max_by_key(|c| c.level);
    let (value, caption) = match first {
        Some(c) => (
            Span::styled(
                format!("{n} to look at"),
                big(if c.level == Level::Bad {
                    t.bad()
                } else {
                    t.warn()
                }),
            ),
            c.title.clone(),
        ),
        None => (
            Span::styled("all clear", big(t.ok())),
            "every check passes".to_string(),
        ),
    };
    let short = if n == 0 {
        ("clear", "all checks")
    } else {
        (&*n.to_string(), "to look at")
    };
    tile(f, d, app, "Attention", value, caption, (short.0, short.1));
    hits.add(d, Action::Go(Page::Health));
}

fn use_next(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits, agents: &[Harness]) {
    let t = &app.theme;
    let inner = card(f, area, "Use next", t, None);
    let w = inner.width as usize;
    let agent_w = 13;
    let name_w = (w / 5).clamp(10, 22);
    for (i, h) in agents.iter().enumerate() {
        let rect = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        if rect.y >= inner.y + inner.height {
            break;
        }
        let mut spans = vec![
            Span::styled(format!(" {DOT} "), Style::default().fg(t.harness(*h))),
            Span::styled(fit(h.name(), agent_w), t.s_dim()),
        ];
        let picks = app.derived.picks.get(h).map(Vec::as_slice).unwrap_or(&[]);
        match app
            .best(*h)
            .and_then(|p| app.reg.get(&p.key).map(|a| (p, a)))
        {
            Some((pick, acc)) => {
                let cmd = launch_hint(app, acc);
                // On a narrow terminal the reason matters more than the
                // command, which the Accounts page also shows.
                let cmd_w = if w < 72 { 0 } else { width(&cmd).min(w / 3) };
                let room = w.saturating_sub(3 + agent_w + 2 + name_w + 1 + cmd_w + 2);
                spans.push(Span::styled(format!("{STAR} "), t.s(t.accent())));
                spans.push(Span::styled(fit(&acc.title(), name_w), t.s_bold()));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(fit(&pick.headline, room), t.s_dim()));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(fit_right(&cmd, cmd_w), t.s(t.accent())));
                hits.add(rect, Action::AccountMenu(acc.key()));
            }
            None => {
                let why = picks
                    .first()
                    .map(|p| p.headline.clone())
                    .unwrap_or_else(|| "no account".to_string());
                let room = w.saturating_sub(3 + agent_w);
                spans.push(Span::styled(
                    fit(&format!("nothing usable right now · {why}"), room),
                    t.s(t.warn()),
                ));
            }
        }
        f.render_widget(Paragraph::new(Line::from(spans)), rect);
    }
}

/// Column widths of the accounts table for an inner width of `w`.
struct Cols {
    name: usize,
    plan: usize,
    bar: usize,
    reset: usize,
    status: usize,
}

fn cols(w: usize) -> Cols {
    let name = (w / 4).clamp(12, 26);
    let plan = if w >= 72 { 10 } else { 0 };
    let status = if w >= 88 { 13 } else { 0 };
    let reset = if w >= 118 { 30 } else { 9 };
    let gaps = |n: usize| if n > 0 { n + 1 } else { 0 };
    // marker, dot, name, percent and the spaces between.
    let fixed = 3 + name + 1 + gaps(plan) + 5 + 2 + reset + gaps(status);
    Cols {
        name,
        plan,
        bar: w.saturating_sub(fixed).clamp(6, 34),
        reset,
        status,
    }
}

fn account_row(
    app: &App,
    acc: &Account,
    c: &Cols,
    selected: bool,
    now: DateTime<Utc>,
) -> Line<'static> {
    let t = &app.theme;
    let (mark, _) = row_style(app, selected);
    let mut spans = vec![mark, dot(app, acc)];
    spans.extend(name_cell(app, acc, c.name, selected, true));
    spans.push(Span::raw(" "));
    if c.plan > 0 {
        spans.push(Span::styled(fit(&plan_text(app, acc), c.plan), t.s_dim()));
        spans.push(Span::raw(" "));
    }
    let signed_in = app.derived.logged_in.contains(&acc.key());
    match acc.snapshot().and_then(main_window).filter(|_| signed_in) {
        Some(w) => {
            let used = w.used_now(now);
            spans.extend(widgets::bar(t, used, c.bar, w.elapsed_pct(now)));
            spans.push(Span::raw(" "));
            spans.push(widgets::pct_span(t, used));
            spans.push(Span::raw("  "));
            let reset = match w.resets_at {
                Some(at) if at > now && c.reset >= 30 => format!(
                    "{} · in {}",
                    model::landing(at, now, app.settings.clock_24h),
                    model::span(at - now)
                ),
                Some(at) if at > now => model::span(at - now),
                Some(_) => "reset".to_string(),
                None => "–".to_string(),
            };
            spans.push(Span::styled(fit(&reset, c.reset), t.s_dim()));
        }
        None => {
            let (text, color) = no_data(app, acc);
            spans.push(Span::styled(
                fit(&text, c.bar + 1 + 4 + 2 + c.reset),
                Style::default().fg(color),
            ));
        }
    }
    if c.status > 0 {
        spans.push(Span::raw(" "));
        spans.push(status_span(t, status(app, acc)));
    }
    Line::from(spans)
}

fn table(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits, now: DateTime<Utc>) {
    let t = &app.theme;
    let focused = app.focus == Focus::Main;
    let inner = card(f, area, "Accounts", t, focused.then(|| t.accent()));
    if inner.height < 2 {
        return;
    }
    let c = cols(inner.width as usize);
    let mut header = vec![Span::raw("   "), head(t, "ACCOUNT", c.name + 1)];
    if c.plan > 0 {
        header.push(head(t, "PLAN", c.plan + 1));
    }
    header.push(head(t, "THIS WEEK", c.bar + 1 + 4 + 2));
    header.push(head(
        t,
        if c.reset >= 30 { "RESETS" } else { "RESETS IN" },
        c.reset,
    ));
    if c.status > 0 {
        header.push(Span::raw(" "));
        header.push(head(t, "STATE", c.status));
    }
    f.render_widget(
        Paragraph::new(Line::from(header)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let list_h = inner.height as usize - 1;
    let sel = app.row();
    let start = scroll_start(sel, app.reg.accounts.len(), list_h);
    for (i, acc) in app.reg.accounts.iter().enumerate().skip(start).take(list_h) {
        let rect = Rect::new(inner.x, inner.y + 1 + (i - start) as u16, inner.width, 1);
        let selected = focused && i == sel;
        if selected {
            paint_row(f, rect, Style::default().bg(t.raised()));
        }
        f.render_widget(
            Paragraph::new(account_row(app, acc, &c, selected, now)),
            rect,
        );
        hits.add(rect, Action::Row(i));
    }
}
