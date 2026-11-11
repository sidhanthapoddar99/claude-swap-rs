//! Accounts: the list on the left, everything about the selected one on the
//! right. Adding, signing in, naming and removing all start here.

use chrono::{DateTime, Utc};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{
    dot, launch_hint, main_window, name_cell, no_data, paint_row, plan_text, row_style,
    scroll_start, status, status_span, window_cells, Status,
};
use crate::harness::Harness;
use crate::model::{self, Source};
use crate::paths;
use crate::store::{self, Account};
use crate::tui::app::{Action, App, Focus};
use crate::tui::theme::DOT;
use crate::tui::widgets::{card, fit, kv, kv_wrapped, truncate, Hits};

const LABEL_W: usize = 11;

enum Item {
    Header(Harness),
    Account(usize),
    Add,
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let list_w = (area.width * 2 / 5).clamp(28, 40);
    let [left, right] =
        Layout::horizontal([Constraint::Length(list_w), Constraint::Min(20)]).areas(area);
    list(f, left, app, hits);
    match app.reg.accounts.get(app.row()) {
        Some(acc) => detail(f, right, app, acc),
        None => {
            let inner = card(f, right, "", &app.theme, None);
            super::empty(f, inner, app, "No accounts yet", &["Press a to add one."]);
        }
    }
}

fn list(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let focused = app.focus == Focus::Main;
    let inner = card(f, area, "", t, focused.then(|| t.accent()));
    let w = inner.width as usize;

    let mut items = Vec::new();
    let mut last = None;
    for (i, acc) in app.reg.accounts.iter().enumerate() {
        if last != Some(acc.harness) {
            items.push(Item::Header(acc.harness));
            last = Some(acc.harness);
        }
        items.push(Item::Account(i));
    }
    items.push(Item::Add);

    let sel = app.row();
    let sel_line = items
        .iter()
        .position(|it| matches!(it, Item::Account(i) if *i == sel))
        .unwrap_or(0);
    let h = inner.height as usize;
    // Keep the group header of the first visible account on screen.
    let start = scroll_start(sel_line, items.len(), h);
    for (line, item) in items.iter().enumerate().skip(start).take(h) {
        let rect = Rect::new(inner.x, inner.y + (line - start) as u16, inner.width, 1);
        match item {
            Item::Header(hn) => {
                let installed = app.derived.bins.get(hn).is_some_and(|b| b.is_some());
                let mut spans = vec![
                    Span::styled(format!(" {DOT} "), Style::default().fg(t.harness(*hn))),
                    Span::styled(
                        hn.name().to_uppercase(),
                        Style::default().fg(t.dim()).add_modifier(Modifier::BOLD),
                    ),
                ];
                if !installed {
                    spans.push(Span::styled("  not installed", t.s_faint()));
                } else if hn.spec().experimental {
                    spans.push(Span::styled("  experimental", t.s_faint()));
                }
                f.render_widget(Paragraph::new(Line::from(spans)), rect);
            }
            Item::Account(i) => {
                let acc = &app.reg.accounts[*i];
                let selected = focused && *i == sel;
                let (mark, bg) = row_style(app, selected || *i == sel);
                if *i == sel {
                    paint_row(f, rect, bg);
                }
                let state = status(app, acc);
                let state_w = 12.min(w / 3);
                let mut spans = vec![mark, Span::raw("  ")];
                spans.extend(name_cell(
                    app,
                    acc,
                    w.saturating_sub(4 + state_w),
                    selected,
                    false,
                ));
                spans.push(Span::raw(" "));
                let s = status_span(t, state);
                spans.push(Span::styled(fit(&s.content, state_w), s.style));
                f.render_widget(Paragraph::new(Line::from(spans)), rect);
                hits.add(rect, Action::Row(*i));
            }
            Item::Add => {
                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled("   + ", t.s_accent()),
                        Span::styled("Add account", t.s_accent()),
                        Span::styled("  a", t.s_faint()),
                    ])),
                    rect,
                );
                hits.add(rect, Action::AddAccount);
            }
        }
    }
}

fn days(n: i64) -> String {
    match n {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        n => format!("in {n} days"),
    }
}

/// How long the login lasts before the vendor wants a fresh sign-in.
fn login_line(app: &App, acc: &Account, now: DateTime<Utc>) -> Vec<Span<'static>> {
    let t = &app.theme;
    let key = acc.key();
    if !app.derived.logged_in.contains(&key) {
        return vec![Span::styled("not signed in", t.s_faint())];
    }
    let local = app.derived.local.get(&key);
    match local.and_then(|l| l.login_expires) {
        Some(at) if at <= now => vec![Span::styled(
            format!("expired {}: sign in again (l)", model::ago(at, now)),
            t.s(t.bad()),
        )],
        Some(at) => {
            let left = at - now;
            let color = if left.num_days() < 3 {
                t.bad()
            } else if left.num_days() < 10 {
                t.warn()
            } else {
                t.text()
            };
            vec![
                Span::styled(
                    format!("good for {}", model::span(left)),
                    Style::default().fg(color),
                ),
                Span::styled(
                    format!(
                        " · until {}, then sign in again",
                        at.with_timezone(&chrono::Local).format("%a %-d %b")
                    ),
                    t.s_dim(),
                ),
            ]
        }
        None => vec![Span::styled(
            acc.harness.spec().login_life.to_string(),
            t.s_dim(),
        )],
    }
}

/// The short-lived token under the login. The agent renews it; cswap never
/// does.
fn token_line(app: &App, acc: &Account, now: DateTime<Utc>) -> Option<Vec<Span<'static>>> {
    let t = &app.theme;
    let at = app.derived.local.get(&acc.key())?.token_expires?;
    let bin = acc.harness.spec().bins[0];
    Some(if at > now {
        vec![
            Span::styled(format!("fresh for {}", model::span(at - now)), t.s_text()),
            Span::styled(format!(" · `{bin}` renews it by itself"), t.s_dim()),
        ]
    } else {
        vec![
            Span::styled(format!("ran out {}", model::ago(at, now)), t.s(t.warn())),
            Span::styled(
                format!(" · `{bin}` renews it the next time it runs here"),
                t.s_dim(),
            ),
        ]
    })
}

fn renewal_line(app: &App, acc: &Account) -> Vec<Span<'static>> {
    let t = &app.theme;
    let b = &acc.meta.billing;
    let today = store::today();
    let Some(date) = b.next(today) else {
        return vec![Span::styled(
            "not set · press b to add the date",
            t.s_faint(),
        )];
    };
    let left = (date - today).num_days();
    let when = date.format("%a %-d %b %Y").to_string();
    let by = match b.source {
        Source::Api => "from the vendor",
        Source::Manual => "set by hand",
    };
    if b.cancelled {
        let (text, color) = if left < 0 {
            (format!("ended {when}"), t.bad())
        } else {
            (format!("ends {when} ({})", days(left)), t.warn())
        };
        return vec![
            Span::styled(text, Style::default().fg(color)),
            Span::styled(" · cancelled".to_string(), t.s_dim()),
        ];
    }
    let cycle = b.cycle.map(|c| c.label()).unwrap_or("one date");
    vec![
        Span::styled(format!("{cycle} · next {when}"), t.s_text()),
        Span::styled(format!(" ({}) · {by}", days(left)), t.s_dim()),
    ]
}

fn detail(f: &mut Frame, area: Rect, app: &App, acc: &Account) {
    let t = &app.theme;
    let now = Utc::now();
    let key = acc.key();
    let inner = card(f, area, "", t, None);
    let w = inner.width as usize;
    let room = w.saturating_sub(LABEL_W);
    let signed_in = app.derived.logged_in.contains(&key);
    let state = status(app, acc);
    let plain = |s: String| vec![Span::styled(truncate(&s, room), t.s_text())];
    let dim = |s: String| vec![Span::styled(truncate(&s, room), t.s_dim())];

    let mut lines: Vec<Line> = Vec::new();

    // Who it is.
    let state_span = status_span(t, state);
    let name = acc.title();
    let name_w = w.saturating_sub(crate::tui::widgets::width(&state_span.content) + 3);
    lines.push(Line::from(vec![
        dot(app, acc),
        Span::styled(
            fit(&name, name_w),
            Style::default().fg(t.text()).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        state_span,
    ]));
    let mut sub = format!("{key} · {}", acc.harness.name());
    if acc.is_native() {
        sub.push_str(" · the agent's own login");
    }
    if app.is_default(acc) {
        sub.push_str(" · default");
    }
    lines.push(Line::from(Span::styled(
        format!("  {}", truncate(&sub, w.saturating_sub(2))),
        t.s_faint(),
    )));
    lines.push(Line::default());

    // The login.
    let id = &acc.meta.identity;
    let mut who = id.email.clone().unwrap_or_default();
    if let Some(org) = id.org.as_ref().filter(|o| !o.is_empty()) {
        if !who.is_empty() {
            who.push_str(" · ");
        }
        who.push_str(org);
    }
    if who.is_empty() {
        who = if signed_in {
            "signed in (the login names no one)"
        } else {
            "–"
        }
        .to_string();
    }
    lines.push(kv(t, "Login", LABEL_W, plain(who)));
    let plan = plan_text(app, acc);
    lines.push(kv(
        t,
        "Plan",
        LABEL_W,
        if plan.is_empty() {
            vec![Span::styled("unknown · press p to name it", t.s_faint())]
        } else if acc.meta.plan_source == Source::Manual && acc.meta.plan.is_some() {
            vec![
                Span::styled(plan, t.s_text()),
                Span::styled(" · set by hand", t.s_dim()),
            ]
        } else {
            plain(plan)
        },
    ));
    lines.extend(kv_wrapped(
        t,
        "Renewal",
        LABEL_W,
        renewal_line(app, acc),
        room,
    ));
    lines.extend(kv_wrapped(
        t,
        "Sign-in",
        LABEL_W,
        login_line(app, acc, now),
        room,
    ));
    if signed_in {
        if let Some(spans) = token_line(app, acc, now) {
            lines.extend(kv_wrapped(t, "Token", LABEL_W, spans, room));
        }
    }
    lines.push(Line::default());

    // Usage, in one line; the Usage page has the rest.
    match acc.snapshot().and_then(main_window).filter(|_| signed_in) {
        Some(win) => {
            let bar_w = room.saturating_sub(5 + 2 + 32).clamp(6, 28);
            lines.push(kv(
                t,
                &format!("This {}", win.period()),
                LABEL_W,
                window_cells(app, win, bar_w, now),
            ));
        }
        None => {
            let (text, color) = no_data(app, acc);
            lines.push(kv(
                t,
                "Usage",
                LABEL_W,
                vec![Span::styled(
                    truncate(&text, room),
                    Style::default().fg(color),
                )],
            ));
        }
    }
    let grants = acc.grants(now);
    if !grants.is_empty() {
        let n: u32 = grants.iter().map(|g| g.count).sum();
        let mut text = format!("{n} spare");
        if let Some(at) = grants.iter().filter_map(|g| g.expires_at).min() {
            text.push_str(&format!(
                " · the first expires in {}",
                model::span(at - now)
            ));
        }
        lines.push(kv(
            t,
            "Resets",
            LABEL_W,
            vec![Span::styled(truncate(&text, room), t.s(t.info()))],
        ));
    }
    if let Some(pick) = app
        .derived
        .picks
        .get(&acc.harness)
        .and_then(|p| p.iter().find(|p| p.key == key))
    {
        let best = app.best(acc.harness).is_some_and(|b| b.key == key);
        let text = pick.reasons.join(" · ");
        let mut spans = Vec::new();
        if best {
            spans.push(Span::styled("best pick · ", t.s(t.accent())));
        }
        spans.push(Span::styled(
            text,
            if pick.usable {
                t.s_dim()
            } else {
                t.s(t.warn())
            },
        ));
        lines.extend(kv_wrapped(t, "Advice", LABEL_W, spans, room));
    }
    lines.push(Line::default());

    // How to use it and where it lives.
    lines.push(kv(
        t,
        "Launch",
        LABEL_W,
        vec![Span::styled(launch_hint(app, acc), t.s(t.accent()))],
    ));
    let folder = if acc.is_native() {
        acc.harness.native_dir()
    } else {
        acc.dir()
    };
    lines.push(kv(t, "Folder", LABEL_W, dim(paths::tilde(&folder))));
    if !acc.is_native() {
        let s = &acc.meta.sync;
        let mut text = format!("{} linked · {} private", s.linked, s.private);
        if !s.shadowed.is_empty() {
            text.push_str(&format!(" · {} shadowed", s.shadowed.len()));
        }
        if let Some(at) = s.last_synced {
            text.push_str(&format!(" · synced {}", model::ago(at, now)));
        }
        lines.push(kv(
            t,
            "Shared",
            LABEL_W,
            vec![Span::styled(
                truncate(&text, room),
                if s.shadowed.is_empty() {
                    t.s_dim()
                } else {
                    t.s(t.warn())
                },
            )],
        ));
    }
    let mut used = match acc.meta.last_used {
        Some(at) => model::ago(at, now),
        None if acc.is_native() => "not tracked for the agent's own login".to_string(),
        None => "never through cswap".to_string(),
    };
    if let Status::Live(_) = state {
        let pids: Vec<String> = app
            .live
            .get(&key)
            .map(|p| p.iter().map(|p| p.to_string()).collect())
            .unwrap_or_default();
        used = format!("running now · pid {}", pids.join(", "));
    }
    lines.push(kv(t, "Last used", LABEL_W, dim(used)));

    f.render_widget(Paragraph::new(lines), inner);
}
