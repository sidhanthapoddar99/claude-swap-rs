//! Accounts: the list on the left, everything about the selected one on the
//! right. Adding, signing in, naming and removing all start here.
//!
//! The right side has four parts. First the facts, a line each. Then every
//! limit of the account and its pay-as-you-go credit, then the spare resets
//! it holds. Last, the details: where the account lives, what each fact
//! means and which key changes it.

use chrono::{DateTime, Utc};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Borders, Paragraph};
use ratatui::Frame;

use super::{
    dot, expiry_color, fit_date, head, launch_hint, limit_lines, name_cell, paint_row, plan_text,
    row_style, scroll_start, status, status_span, LimitCols,
};
use crate::harness::Harness;
use crate::model::{self, Source};
use crate::paths;
use crate::store::{self, Account};
use crate::tui::app::{Action, App, Focus};
use crate::tui::theme::{DOT, STAR};
use crate::tui::widgets::{
    self, card, card_note, card_with, clip, fit, kv, kv_wrapped, truncate, Hits,
};
use crate::view;

const LABEL_W: usize = 11;

enum Item {
    Header(Harness),
    Account(usize),
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
            super::no_accounts(f, inner, app);
        }
    }
}

fn list(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let focused = app.focus == Focus::Main;
    let inner = card(f, area, "", t, focused.then(|| t.accent()));
    let w = inner.width as usize;
    if inner.height == 0 {
        return;
    }

    let mut items = Vec::new();
    let mut last = None;
    for (i, acc) in app.reg.accounts.iter().enumerate() {
        if last != Some(acc.harness) {
            items.push(Item::Header(acc.harness));
            last = Some(acc.harness);
        }
        items.push(Item::Account(i));
    }

    // The last line of the card is the add button, with a blank line above
    // it when there is room. The accounts scroll in what is left.
    let add = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" + ", t.s_accent()),
            Span::styled("Add account", t.s_accent()),
            Span::styled("  a", t.s_faint()),
        ])),
        add,
    );
    hits.add(add, Action::AddAccount);

    let sel = app.row();
    let sel_line = items
        .iter()
        .position(|it| matches!(it, Item::Account(i) if *i == sel))
        .unwrap_or(0);
    let h = (inner.height as usize).saturating_sub(if inner.height >= 4 { 2 } else { 1 });
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
                // Where a launch that names no account goes, when it is not
                // one of the accounts below: those wear a star.
                let start = app.hs.get(hn).map(|s| s.start.as_str());
                let note = match start {
                    _ if !installed => "not installed",
                    Some("auto") => "default: auto",
                    Some("root") => "default: root",
                    _ if hn.spec().experimental => "experimental",
                    _ => "",
                };
                if !note.is_empty() {
                    let used = widgets::line_width(&Line::from(spans.clone()));
                    let room = w.saturating_sub(used + 1);
                    if widgets::width(note) <= room {
                        spans.push(Span::raw(" ".repeat(room - widgets::width(note))));
                        spans.push(Span::styled(note, t.s_faint()));
                    }
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
                // The default account of the agent has a star before its name.
                let star = if app.is_default(acc) {
                    Span::styled(format!("{STAR} "), t.s(t.harness(acc.harness)))
                } else {
                    Span::raw("  ")
                };
                let mut spans = vec![mark, star];
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
        }
    }
}

fn days(n: i64) -> String {
    match n {
        0 => "today".to_string(),
        1 => "in 1 day".to_string(),
        n => format!("in {n} days"),
    }
}

// ------------------------------------------------------------------ the facts

/// Is the login signed in, and until when. One short line.
fn sign_in_fact(app: &App, acc: &Account, now: DateTime<Utc>) -> Vec<Span<'static>> {
    let t = &app.theme;
    let key = acc.key();
    if !app.derived.logged_in.contains(&key) {
        return vec![Span::styled("not signed in", t.s_faint())];
    }
    match app.derived.local.get(&key).and_then(|l| l.login_expires) {
        Some(at) if at <= now => vec![Span::styled(
            format!("expired {}", model::ago(at, now)),
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
                    format!("good for {}", model::until(at, now)),
                    Style::default().fg(color),
                ),
                Span::styled(format!(" · until {}", model::day(at)), t.s_dim()),
            ]
        }
        None => vec![Span::styled("signed in", t.s_text())],
    }
}

/// The short-lived token under the login: how long it is still good for.
fn token_fact(app: &App, acc: &Account, now: DateTime<Utc>) -> Option<Vec<Span<'static>>> {
    let t = &app.theme;
    let at = app.derived.local.get(&acc.key())?.token_expires?;
    Some(vec![if at > now {
        Span::styled(format!("fresh for {}", model::until(at, now)), t.s_text())
    } else {
        Span::styled(format!("ran out {}", model::ago(at, now)), t.s(t.warn()))
    }])
}

/// The renewal date and how many days it is off.
fn renewal_fact(app: &App, acc: &Account) -> Vec<Span<'static>> {
    let t = &app.theme;
    let b = &acc.meta.billing;
    let today = store::today();
    let Some(date) = b.next(today) else {
        return vec![Span::styled("not set", t.s_faint())];
    };
    let left = (date - today).num_days();
    let day = view::day_of(date);
    match (b.cancelled, left < 0) {
        (true, true) => vec![Span::styled(format!("ended {day}"), t.s(t.bad()))],
        (true, false) => vec![
            Span::styled(format!("ends {day}"), t.s(t.warn())),
            Span::styled(format!(" · {}", days(left)), t.s_dim()),
        ],
        _ => vec![
            Span::styled(day, t.s_text()),
            Span::styled(format!(" · {}", days(left)), t.s_dim()),
        ],
    }
}

/// The facts about an account, one line each.
fn info(
    app: &App,
    acc: &Account,
    now: DateTime<Utc>,
    w: usize,
    spaced: bool,
) -> Vec<Line<'static>> {
    let t = &app.theme;
    let key = acc.key();
    let room = w.saturating_sub(LABEL_W);
    let signed_in = app.derived.logged_in.contains(&key);
    let plain = |s: String| vec![Span::styled(truncate(&s, room), t.s_text())];
    let dim = |s: String| vec![Span::styled(truncate(&s, room), t.s_dim())];
    let faint = |s: &str| vec![Span::styled(truncate(s, room), t.s_faint())];
    let mut lines: Vec<Line> = Vec::new();
    let gap = |lines: &mut Vec<Line<'static>>| {
        if spaced {
            lines.push(Line::default());
        }
    };

    // Who it is.
    let id = &acc.meta.identity;
    let mut who = id.email.clone().unwrap_or_default();
    if let Some(org) = id.org.as_ref().filter(|o| !o.is_empty()) {
        if !who.is_empty() {
            who.push_str(" · ");
        }
        who.push_str(org);
    }
    if who.is_empty() {
        who = if signed_in { "signed in" } else { "–" }.to_string();
    }
    lines.push(kv(t, "Login", LABEL_W, plain(who)));
    let plan = plan_text(app, acc);
    lines.push(kv(
        t,
        "Plan",
        LABEL_W,
        if plan.is_empty() {
            faint("unknown")
        } else {
            plain(plan)
        },
    ));
    lines.push(kv(
        t,
        "Aliases",
        LABEL_W,
        if acc.meta.aliases.is_empty() {
            faint("none")
        } else {
            plain(acc.meta.aliases.join(", "))
        },
    ));
    gap(&mut lines);

    // What runs out when.
    lines.push(kv(
        t,
        "Renewal",
        LABEL_W,
        clip(renewal_fact(app, acc), room),
    ));
    lines.push(kv(
        t,
        "Sign-in",
        LABEL_W,
        clip(sign_in_fact(app, acc, now), room),
    ));
    if signed_in {
        if let Some(spans) = token_fact(app, acc, now) {
            lines.push(kv(t, "Token", LABEL_W, clip(spans, room)));
        }
    }
    gap(&mut lines);

    // How to start it.
    lines.push(kv(
        t,
        "Launch",
        LABEL_W,
        vec![Span::styled(
            truncate(&launch_hint(app, acc), room),
            t.s(t.accent()),
        )],
    ));
    let used = match acc.meta.last_used {
        Some(at) => model::ago(at, now),
        None => "never".to_string(),
    };
    lines.push(kv(t, "Last used", LABEL_W, dim(used)));
    lines
}

// ------------------------------------------------------------------ the usage

/// Every limit of the account, a line each, the same as on the Usage page.
/// The header comes apart from the lines, as it does for the resets.
fn usage(
    app: &App,
    acc: &Account,
    now: DateTime<Utc>,
    w: usize,
) -> (Option<Line<'static>>, Vec<Line<'static>>) {
    let mut c = LimitCols::new(w, 24, app.settings.clock_24h);
    // The bar is the first thing to go on a card too narrow for the line.
    if c.width() > w {
        c.bar = 0;
    }
    let signed_in = app.derived.logged_in.contains(&acc.key());
    let top = acc
        .snapshot()
        .filter(|s| signed_in && !s.windows.is_empty())
        .map(|_| Line::from(clip(c.header(&app.theme), w)));
    let lines = limit_lines(app, acc, &c, now)
        .into_iter()
        .map(|cells| Line::from(clip(cells, w)))
        .collect();
    (top, lines)
}

// ----------------------------------------------------------------- the resets

/// Every spare reset the account holds, a line each. The header of the
/// table comes apart from it: a card with too little room leaves the header
/// out and keeps the resets.
fn resets(
    app: &App,
    acc: &Account,
    now: DateTime<Utc>,
    w: usize,
) -> (Option<Line<'static>>, Vec<Line<'static>>) {
    let t = &app.theme;
    let mut top = None;
    let mut lines: Vec<Line> = Vec::new();
    let rows = view::grants_of(acc, now);
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            truncate("No spare reset held right now.", w),
            t.s_faint(),
        )));
    } else {
        // count, what resets, how long it is still good for, and the date.
        let wide = w >= 40;
        let (count, gap) = if wide { (5, 2) } else { (2, 1) };
        let what = if wide { 10 } else { 8 };
        let left = if wide { 9 } else { 7 };
        let date = w.saturating_sub(count + gap + what + 1 + left + 1);
        let date = if date >= 10 { date } else { 0 };
        let mut header = vec![
            head(
                t,
                &format!("{:>count$}", if wide { "COUNT" } else { "#" }),
                count + gap,
            ),
            head(t, "RESET", what + 1),
            head(t, "EXPIRES", left + 1),
        ];
        if date > 0 {
            header.push(head(t, "DATE", date));
        }
        top = Some(Line::from(header));
        for row in &rows {
            let n = row.count.unwrap_or(1);
            let (when, on) = match row.at {
                Some(_) => (
                    row.countdown(now),
                    fit_date(row.date(now, app.settings.clock_24h), date),
                ),
                None => ("–".to_string(), "does not expire".to_string()),
            };
            let mut spans = vec![
                Span::styled(format!("{n:>count$}{}", " ".repeat(gap)), t.s_text()),
                Span::styled(fit(&row.what, what + 1), t.s(t.info())),
                Span::styled(
                    fit(&when, left + 1),
                    Style::default().fg(expiry_color(t, row.at, now)),
                ),
            ];
            if date > 0 {
                spans.push(Span::styled(fit(&on, date), t.s_dim()));
            }
            lines.push(Line::from(spans));
        }
    }
    (top, lines)
}

// ------------------------------------------------------------- the wordy part

/// What the facts mean and which key changes each. Everything long goes
/// here, so the facts above stay a line each.
fn details(app: &App, acc: &Account, now: DateTime<Utc>, w: usize) -> Vec<Line<'static>> {
    let t = &app.theme;
    let key = acc.key();
    let h = acc.harness;
    let bin = h.spec().bins[0];
    let room = w.saturating_sub(LABEL_W);
    let signed_in = app.derived.logged_in.contains(&key);
    let local = app.derived.local.get(&key);
    let mut lines: Vec<Line> = Vec::new();
    let mut say = |label: &str, spans: Vec<Span<'static>>| {
        lines.extend(kv_wrapped(t, label, LABEL_W, spans, room));
    };
    let dim = |s: String| vec![Span::styled(s, t.s_dim())];

    // Where it stands among the agent's accounts.
    if let Some(pick) = app
        .derived
        .picks
        .get(&h)
        .and_then(|p| p.iter().find(|p| p.key == key))
    {
        let mut spans = Vec::new();
        if app.best(h).is_some_and(|b| b.key == key) {
            spans.push(Span::styled("best pick · ", t.s(t.accent())));
        }
        spans.push(Span::styled(
            pick.reasons.join(" · "),
            if pick.usable {
                t.s_dim()
            } else {
                t.s(t.warn())
            },
        ));
        say("Advice", spans);
    }
    if app.is_default(acc) {
        say(
            "Default",
            vec![
                Span::styled(format!("{STAR} "), t.s(t.harness(h))),
                Span::styled(format!("`cswap {bin}` starts here."), t.s_dim()),
            ],
        );
    }

    // Where it lives.
    say("Folder", dim(paths::tilde(&acc.dir())));
    {
        let s = &acc.meta.sync;
        let mut text = format!("{} linked · {} private", s.linked, s.private);
        if !s.shadowed.is_empty() {
            text.push_str(&format!(" · {} shadowed", s.shadowed.len()));
        }
        if let Some(at) = s.last_synced {
            text.push_str(&format!(" · synced {}", model::ago(at, now)));
        }
        say(
            "Shared",
            vec![Span::styled(
                text,
                if s.shadowed.is_empty() {
                    t.s_dim()
                } else {
                    t.s(t.warn())
                },
            )],
        );
    }

    // The login and the token under it.
    let expired = local
        .and_then(|l| l.login_expires)
        .is_some_and(|at| at <= now);
    say(
        "Sign-in",
        if !signed_in {
            dim(format!(
                "Press l to sign in. {}'s own sign-in runs in this terminal.",
                h.name()
            ))
        } else if expired {
            vec![Span::styled(
                "The login has run out. Press l to sign in again.".to_string(),
                t.s(t.bad()),
            )]
        } else {
            dim(h.spec().login_life.to_string())
        },
    );
    if signed_in
        && local
            .and_then(|l| l.token_expires)
            .is_some_and(|at| at <= now)
    {
        say(
            "Token",
            dim(format!(
                "`{bin}` renews it the next time it runs on this account. cswap never does."
            )),
        );
    }

    // The plan and its renewal.
    let b = &acc.meta.billing;
    say(
        "Renewal",
        match b.next(store::today()) {
            None => dim("No date is set. Press b to add one.".to_string()),
            Some(_) if b.cancelled => dim(
                "Cancelled: the plan ends on that date and does not renew. b changes it."
                    .to_string(),
            ),
            Some(_) => dim(format!(
                "{} · {} · b changes it",
                b.cycle.map(|c| c.label()).unwrap_or("one date"),
                match b.source {
                    Source::Api => "from the vendor",
                    Source::Manual => "set by hand",
                }
            )),
        },
    );
    if plan_text(app, acc).is_empty() {
        say(
            "Plan",
            dim("The vendor does not report it. Press p to name it.".to_string()),
        );
    } else if acc.meta.plan_source == Source::Manual && acc.meta.plan.is_some() {
        say("Plan", dim("Set by hand. p changes it.".to_string()));
    }
    if acc.meta.aliases.is_empty() {
        say("Aliases", dim("None yet. Press E to add some.".to_string()));
    }
    lines
}

// ------------------------------------------------------------------ the cards

/// How tall each of four cards is, top to bottom, in `avail` lines. `want`
/// is what each asks for, borders included; the last one takes what is left.
/// A card that cannot have three lines is left out, and so is every card
/// after it. Whatever is over goes to the last card shown.
fn heights(want: [u16; 3], avail: u16) -> [u16; 4] {
    let mut out = [0u16; 4];
    let mut left = avail;
    for (i, need) in want.into_iter().enumerate() {
        // The first card is always there, however little room it gets.
        if i > 0 && left < 3 {
            break;
        }
        out[i] = need.min(left);
        left -= out[i];
    }
    let shown = out.iter().take(3).all(|h| *h > 0);
    if shown && left >= 3 {
        out[3] = left;
    } else if let Some(last) = out.iter_mut().rev().find(|h| **h > 0) {
        *last += left;
    }
    out
}

fn detail(f: &mut Frame, area: Rect, app: &App, acc: &Account) {
    let t = &app.theme;
    let now = Utc::now();
    if area.width < 8 || area.height < 3 {
        return;
    }
    // Every card has a border and a cell of padding on each side.
    let w = area.width as usize - 4;
    let (usage_head, mut usage_lines) = usage(app, acc, now, w);
    let usage_n = usage_lines.len() + usize::from(usage_head.is_some());
    let (reset_head, mut reset_lines) = resets(app, acc, now, w);
    let reset_n = reset_lines.len() + usize::from(reset_head.is_some());
    let detail_lines = details(app, acc, now, w);
    // The facts are set apart in groups when all four cards still fit.
    let tight = info(app, acc, now, w, false);
    let all = tight.len() + 2 + usage_n + 2 + reset_n + 2 + detail_lines.len() + 2;
    let info_lines = if all + 2 <= area.height as usize {
        info(app, acc, now, w, true)
    } else {
        tight
    };

    // Top to bottom: the facts, the usage and the resets get what they need,
    // and the wordy part what is left.
    let [info_h, usage_h, resets_h, details_h] = heights(
        [
            info_lines.len() as u16 + 2,
            usage_n as u16 + 2,
            reset_n as u16 + 2,
        ],
        area.height,
    );
    // A table's header is the first thing to go when its card is cut.
    if let Some(header) = usage_head.filter(|_| usage_h as usize >= usage_n + 2) {
        usage_lines.insert(0, header);
    }
    if let Some(header) = reset_head.filter(|_| resets_h as usize >= reset_n + 2) {
        reset_lines.insert(0, header);
    }

    // Who it is, on the border of the first card. A default account has a
    // star in place of its dot.
    let mut title = vec![
        dot(app, acc),
        Span::styled(
            acc.title(),
            Style::default().fg(t.text()).add_modifier(Modifier::BOLD),
        ),
    ];
    let state = status_span(t, status(app, acc));
    let sub = format!("  {} · {}", acc.key(), acc.harness.name());
    let used = widgets::line_width(&Line::from(title.clone())) + widgets::width(&state.content);
    if used + widgets::width(&sub) + 12 <= area.width as usize {
        title.push(Span::styled(sub, t.s_faint()));
    }
    let rect = Rect::new(area.x, area.y, area.width, info_h);
    let inner = card_with(f, rect, Line::from(title), t, t.border(), Borders::ALL);
    card_note(f, rect, Line::from(state), t);
    f.render_widget(Paragraph::new(info_lines), inner);

    let mut y = area.y + info_h;
    if usage_h > 0 {
        let rect = Rect::new(area.x, y, area.width, usage_h);
        let inner = card(f, rect, "Usage", t, None);
        if let Some(at) = acc.snapshot().map(|s| s.fetched_at) {
            card_note(
                f,
                rect,
                Line::from(Span::styled(
                    format!("read {}", model::ago(at, now)),
                    t.s_faint(),
                )),
                t,
            );
        }
        f.render_widget(Paragraph::new(usage_lines), inner);
        y += usage_h;
    }
    if resets_h > 0 {
        let rect = Rect::new(area.x, y, area.width, resets_h);
        let inner = card(f, rect, "Resets", t, None);
        let n: u32 = acc.grants(now).iter().map(|g| g.count).sum();
        if n > 0 {
            card_note(
                f,
                rect,
                Line::from(Span::styled(format!("{n} to spend"), t.s(t.info()))),
                t,
            );
        }
        f.render_widget(Paragraph::new(reset_lines), inner);
        y += resets_h;
    }
    if details_h > 0 {
        let rect = Rect::new(area.x, y, area.width, details_h);
        let inner = card(f, rect, "Details", t, None);
        f.render_widget(Paragraph::new(detail_lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::heights;

    #[test]
    fn the_cards_share_the_height_top_to_bottom() {
        // Room for all: the last card takes what is left.
        assert_eq!(heights([9, 6, 4], 30), [9, 6, 4, 11]);
        // No room for the last: what is over goes to the one before it.
        assert_eq!(heights([9, 6, 4], 21), [9, 6, 6, 0]);
        // A card is cut before it is left out, and left out under three lines.
        assert_eq!(heights([9, 6, 4], 18), [9, 6, 3, 0]);
        assert_eq!(heights([9, 6, 4], 17), [9, 8, 0, 0]);
        // The first card is there on any screen.
        assert_eq!(heights([9, 6, 4], 5), [5, 0, 0, 0]);
        for avail in 3..40 {
            assert_eq!(heights([9, 6, 4], avail).iter().sum::<u16>(), avail);
        }
    }
}
