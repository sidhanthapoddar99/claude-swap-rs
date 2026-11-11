//! Defaults: each agent's own login, the one it uses when cswap is not asked
//! for an account. They are shown here and nowhere else. An account can be
//! made from one: you name it and sign in again.

use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::{head, main_window, no_data, paint_row, plan_text, row_style, window_cells, window_w};
use crate::manage;
use crate::store::Account;
use crate::tui::app::{display_email, Action, App, Focus};
use crate::tui::theme::{DOT, STAR};
use crate::tui::widgets::{card, fit, truncate, Hits};

const AGENT_W: usize = 13;

/// Column widths for an inner width of `w`.
struct Cols {
    login: usize,
    plan: usize,
    bar: usize,
    account: usize,
}

fn cols(w: usize) -> Cols {
    let login = (w / 4).clamp(13, 30);
    let plan = if w >= 96 { 12 } else { 0 };
    let account = if w >= 72 { (w / 8).clamp(9, 16) } else { 0 };
    let gaps = |n: usize| if n > 0 { n + 1 } else { 0 };
    let fixed = 3 + AGENT_W + 1 + login + 1 + gaps(plan) + window_w(0) + gaps(account);
    Cols {
        login,
        plan,
        bar: w.saturating_sub(fixed).clamp(6, 32),
        account,
    }
}

/// The account that holds the same login as this default, when one does.
fn twin<'a>(app: &'a App, acc: &Account) -> Option<&'a Account> {
    manage::duplicate_of(&app.all, acc)
}

fn row(app: &App, acc: &Account, c: &Cols, selected: bool, now: DateTime<Utc>) -> Line<'static> {
    let t = &app.theme;
    let (mark, _) = row_style(app, selected);
    let signed_in = app.derived.logged_in.contains(&acc.key());
    let starts_here = app.is_default(acc);
    let name = acc.harness.name();
    let mut spans = vec![
        mark,
        Span::styled(format!("{DOT} "), t.s(t.harness(acc.harness))),
    ];
    spans.push(Span::styled(
        fit(name, AGENT_W - 2),
        if selected { t.s_bold() } else { t.s_text() },
    ));
    // A star where a bare launch of the agent starts on this login.
    spans.push(if starts_here {
        Span::styled(format!(" {STAR}"), t.s_accent())
    } else {
        Span::raw("  ")
    });
    spans.push(Span::raw(" "));
    let who = if signed_in {
        display_email(acc)
    } else {
        "–".to_string()
    };
    spans.push(Span::styled(fit(&who, c.login), t.s_dim()));
    spans.push(Span::raw(" "));
    if c.plan > 0 {
        spans.push(Span::styled(fit(&plan_text(app, acc), c.plan), t.s_dim()));
        spans.push(Span::raw(" "));
    }
    match acc.snapshot().and_then(main_window).filter(|_| signed_in) {
        Some(w) => spans.extend(window_cells(app, w, c.bar, now)),
        None => {
            let (text, color) = no_data(app, acc);
            spans.push(Span::styled(
                fit(&text, window_w(c.bar)),
                Style::default().fg(color),
            ));
        }
    }
    if c.account > 0 {
        spans.push(Span::raw(" "));
        spans.push(match twin(app, acc) {
            Some(a) => Span::styled(fit(&a.title(), c.account), t.s_text()),
            None => Span::styled(fit("–", c.account), t.s_faint()),
        });
    }
    Line::from(spans)
}

/// What the selected default is for, and what can be made of it.
fn about(app: &App, acc: &Account, now: DateTime<Utc>) -> Vec<Line<'static>> {
    let t = &app.theme;
    let h = acc.harness;
    let bin = h.spec().bins[0];
    let signed_in = app.derived.logged_in.contains(&acc.key());
    let mut lines = Vec::new();

    // Where a bare launch goes.
    let start = app.hs.get(&h).map(|s| s.start.clone()).unwrap_or_default();
    let (text, style) = match start.as_str() {
        "default" | "" => (format!("A bare `{bin}` starts here."), t.s_text()),
        "auto" => (
            format!("A bare `{bin}` starts on the best account, not here · d changes that."),
            t.s_dim(),
        ),
        id => {
            let name = app
                .reg
                .of(h)
                .find(|a| a.meta.id.to_string() == id)
                .map(Account::title)
                .unwrap_or_else(|| format!("account {id}"));
            (
                format!("A bare `{bin}` starts on {name}, not here · d changes that."),
                t.s_dim(),
            )
        }
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("{STAR} "),
            if app.is_default(acc) {
                t.s_accent()
            } else {
                t.s_faint()
            },
        ),
        Span::styled(text, style),
    ]));

    // What can be made of it.
    let line = |key: &'static str, text: String, style: Style| {
        Line::from(vec![
            Span::styled(format!("{key} "), t.s_accent()),
            Span::styled(text, style),
        ])
    };
    match twin(app, acc) {
        _ if !signed_in => lines.push(line(
            " ",
            format!("Not signed in. Run `{bin}` and sign in; it shows up here."),
            t.s_faint(),
        )),
        Some(a) => lines.push(line(
            " ",
            format!("{} holds this login as an account.", a.title()),
            t.s_dim(),
        )),
        None if !h.wrappable() => lines.push(line(
            " ",
            format!("cswap cannot hold accounts for {} yet.", h.name()),
            t.s_faint(),
        )),
        None => lines.push(line(
            "c",
            "makes an account from this login: you name it and sign in once more. A login is never copied, because two copies sign each other out."
                .to_string(),
            t.s_dim(),
        )),
    }
    let spare: u32 = acc.grants(now).iter().map(|g| g.count).sum();
    if spare > 0 {
        lines.push(line(
            " ",
            format!(
                "{spare} spare reset{} held on this login.",
                if spare == 1 { "" } else { "s" }
            ),
            t.s(t.info()),
        ));
    }
    lines
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    let now = Utc::now();
    let rows: Vec<&Account> = app.all.defaults().collect();
    if rows.is_empty() {
        super::empty(
            f,
            area,
            app,
            "No agent found",
            &[
                "cswap looks for claude, codex, agy and grok on your PATH.",
                "Install one, then open cswap again.",
            ],
        );
        return;
    }
    let focused = app.focus == Focus::Main;
    let inner = card(f, area, "", t, focused.then(|| t.accent()));
    if inner.height < 2 {
        return;
    }
    let c = cols(inner.width as usize);
    let mut header = vec![
        Span::raw("   "),
        head(t, "AGENT", AGENT_W + 1),
        head(t, "LOGIN", c.login + 1),
    ];
    if c.plan > 0 {
        header.push(head(t, "PLAN", c.plan + 1));
    }
    header.push(head(t, "USED", c.bar + 1 + 4 + 2));
    header.push(head(t, "RESETS IN", super::IN_W));
    if c.account > 0 {
        header.push(Span::raw(" "));
        header.push(head(t, "ACCOUNT", c.account));
    }
    f.render_widget(
        Paragraph::new(Line::from(header)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let sel = app.row().min(rows.len() - 1);
    let list_h = (inner.height as usize - 1).min(rows.len());
    for (i, acc) in rows.iter().enumerate().take(list_h) {
        let rect = Rect::new(inner.x, inner.y + 1 + i as u16, inner.width, 1);
        let selected = focused && i == sel;
        if selected {
            paint_row(f, rect, Style::default().bg(t.raised()));
        }
        f.render_widget(Paragraph::new(row(app, acc, &c, selected, now)), rect);
        hits.add(rect, Action::Row(i));
    }

    // The selected login in words, under a rule.
    let y = inner.y + 1 + list_h as u16 + 1;
    let end = inner.y + inner.height;
    if y + 2 > end {
        return;
    }
    f.render_widget(
        Paragraph::new(Span::styled(
            "─".repeat(inner.width as usize),
            t.s(t.border()),
        )),
        Rect::new(inner.x, y, inner.width, 1),
    );
    let acc = rows[sel];
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{DOT} "), t.s(t.harness(acc.harness))),
        Span::styled(
            truncate(
                &format!("{}'s default login", acc.harness.name()),
                inner.width as usize - 2,
            ),
            t.s_bold(),
        ),
    ])];
    lines.extend(about(app, acc, now));
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        Rect::new(
            inner.x + 1,
            y + 1,
            inner.width.saturating_sub(2),
            end - y - 1,
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_columns_fit_the_width() {
        for w in 53..200 {
            let c = cols(w);
            let gaps = |n: usize| if n > 0 { n + 1 } else { 0 };
            let used =
                3 + AGENT_W + 1 + c.login + 1 + gaps(c.plan) + window_w(c.bar) + gaps(c.account);
            assert!(used <= w, "{w}: {used}");
        }
    }
}
