//! Settings: one tab of global options, one per agent, one for the shell.

use anyhow::Result;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::{paint_row, row_style, scroll_start};
use crate::harness::Harness;
use crate::health;
use crate::manage;
use crate::paths;
use crate::store::{HarnessSettings, Settings, MIN_REFRESH_MINUTES};
use crate::tui::app::{Action, App, Focus, Modal, Page, Then, Tone};
use crate::tui::theme::{DOT, RING};
use crate::tui::widgets::{card, fit, width, Hits};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    General,
    Agent(Harness),
    Shell,
}

pub const TABS: [Tab; 6] = [
    Tab::General,
    Tab::Agent(Harness::Claude),
    Tab::Agent(Harness::Codex),
    Tab::Agent(Harness::Agy),
    Tab::Agent(Harness::Grok),
    Tab::Shell,
];

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::General => "General",
            // The first word is enough here: `Claude`, `Grok`.
            Tab::Agent(h) => h.name().split(' ').next().unwrap_or(h.name()),
            Tab::Shell => "Shell",
        }
    }
}

/// One thing that can be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Id {
    RefreshMinutes,
    RefreshOnOpen,
    AutoSkip,
    HistoryDays,
    Clock24h,
    Mouse,
    PaintBackground,
    Theme,
    UpdateCheck,
    Wrapped(Harness),
    Default(Harness),
    Announce(Harness),
    SyncEnabled(Harness),
    Exclude(Harness),
    Binary(Harness),
    CliIdentity(Harness),
}

pub struct Row {
    /// `None` for a line that only informs.
    pub id: Option<Id>,
    pub label: String,
    pub value: String,
    pub help: String,
    /// `Some` for an on/off switch.
    pub on: Option<bool>,
}

fn toggle(id: Id, label: &str, on: bool, help: &str) -> Row {
    Row {
        id: Some(id),
        label: label.to_string(),
        value: if on { "on" } else { "off" }.to_string(),
        help: help.to_string(),
        on: Some(on),
    }
}

fn value(id: Id, label: &str, value: String, help: &str) -> Row {
    Row {
        id: Some(id),
        label: label.to_string(),
        value,
        help: help.to_string(),
        on: None,
    }
}

fn note(label: &str, value: String, help: &str) -> Row {
    Row {
        id: None,
        label: label.to_string(),
        value,
        help: help.to_string(),
        on: None,
    }
}

/// What a bare launch of `h` uses, in words.
fn default_text(app: &App, h: Harness, hs: &HarnessSettings) -> String {
    match hs.start.as_str() {
        "default" => "its default login".to_string(),
        "auto" => "auto: the best account".to_string(),
        id => match app.reg.of(h).find(|a| a.meta.id.to_string() == id) {
            Some(acc) => format!("account {}", acc.title()),
            None => format!("{id} (missing)"),
        },
    }
}

pub fn rows(app: &App) -> Vec<Row> {
    let s = &app.settings;
    match TABS[app.settings_tab.min(TABS.len() - 1)] {
        Tab::General => vec![
            value(
                Id::Theme,
                "Theme",
                {
                    let scheme = app.theme.scheme();
                    format!("{} · {}", scheme.name(), scheme.blurb())
                },
                "The dashboard's colours. Violet, blue and green are tinted; dark is true black; grey has no colour at all; terminal uses your terminal's own 16 colours, so it follows the theme you already have.",
            ),
            value(
                Id::RefreshMinutes,
                "Refresh every",
                format!("{} min", s.refresh_minutes),
                "How often usage is fetched while this dashboard is open. A wrapped agent never fetches: it reads what the dashboard last stored.",
            ),
            toggle(
                Id::RefreshOnOpen,
                "Refresh when opened",
                s.refresh_on_open,
                "Fetch whatever is older than the interval as soon as the dashboard opens.",
            ),
            value(
                Id::AutoSkip,
                "Auto-pick skips above",
                format!("{:.0}%", s.auto_skip_above_pct),
                "--cswap=auto and the suggestions leave out an account whose week is this full.",
            ),
            value(
                Id::HistoryDays,
                "Keep history for",
                format!("{} days", s.history_days),
                "How long usage samples are kept for the Analytics page.",
            ),
            toggle(Id::Clock24h, "24-hour clock", s.clock_24h, "Off shows times as 9:30 pm."),
            toggle(
                Id::Mouse,
                "Mouse",
                s.mouse,
                "Click and scroll in the dashboard. Off gives selecting text back to your terminal.",
            ),
            toggle(
                Id::PaintBackground,
                "Paint the background",
                s.paint_background,
                "Off keeps your terminal's own background, for a transparent or a light terminal.",
            ),
            toggle(
                Id::UpdateCheck,
                "Check for updates",
                s.update_check,
                "Looks at GitHub at most once a day and tells you on the command line. It never updates on its own: `cswap update` does that.",
            ),
        ],
        Tab::Agent(h) => {
            let hs = app.hs.get(&h).cloned().unwrap_or_default();
            let spec = h.spec();
            let bin = spec.bins[0];
            let found = match app.derived.bins.get(&h).and_then(|b| b.as_ref()) {
                Some(p) => paths::tilde(p),
                None => "not installed".to_string(),
            };
            let mut out = vec![
                note(
                    "Program",
                    found,
                    "The real program cswap starts. Set a path below when it is not the first one on your PATH.",
                ),
                note(
                    "Its own config dir",
                    paths::tilde(&h.native_dir()),
                    "Where the agent keeps its state. cswap reads it and links to it, and never writes into it.",
                ),
                toggle(
                    Id::Wrapped(h),
                    &format!("Wrap `{bin}`"),
                    hs.wrapped,
                    &format!("On: a bare `{bin}` starts where the next row says. Off: cswap passes straight through unless you give --cswap=."),
                ),
                value(
                    Id::Default(h),
                    &format!("A bare `{bin}` starts on"),
                    default_text(app, h, &hs),
                    &format!("What `{bin}` uses when you name no account: its default login, one of your accounts, or auto, which picks the best account at every launch."),
                ),
                toggle(
                    Id::Announce(h),
                    "Say which account at launch",
                    hs.announce,
                    "One dim line on stderr before the agent starts, and only when cswap chose something other than the agent's default login.",
                ),
                toggle(
                    Id::SyncEnabled(h),
                    "Sync links at launch",
                    hs.sync.enabled,
                    "Bring each account's links up to date with the agent's own config dir every time it starts, so new files there are shared at once.",
                ),
                value(
                    Id::Exclude(h),
                    "Extra private names",
                    if hs.sync.exclude.is_empty() {
                        "none".to_string()
                    } else {
                        hs.sync.exclude.join(", ")
                    },
                    "Names every account keeps to itself, on top of the built-in list. Separate with commas; * matches at either end.",
                ),
                value(
                    Id::Binary(h),
                    "Program path",
                    hs.binary.clone().unwrap_or_else(|| "found on PATH".to_string()),
                    "A full path to the agent's program. Empty goes back to searching PATH.",
                ),
            ];
            if h == Harness::Claude {
                out.push(toggle(
                    Id::CliIdentity(h),
                    "Read spare resets",
                    hs.cli_identity,
                    "Anthropic reports spare resets only to its own CLI. On: cswap names Claude Code in the User-Agent for that one read-only call, at most every 6 hours. Off: no resets are read; note them by hand instead.",
                ));
            }
            if spec.experimental {
                out.push(note(
                    "Status",
                    "experimental".to_string(),
                    "cswap accounts for this agent rely on behaviour its vendor does not document. The agent's default login always works.",
                ));
            }
            out
        }
        Tab::Shell => {
            let (shell, loaded) = health::shell_integration();
            let shell_name = if shell.is_empty() { "your shell".to_string() } else { shell.clone() };
            let state = match loaded {
                Some(true) => "loaded".to_string(),
                Some(false) => "not loaded".to_string(),
                None => "cannot tell for this shell".to_string(),
            };
            let init = crate::cli::shell_hint(&shell);
            let wrapped: Vec<&str> = Harness::ALL
                .into_iter()
                .filter(|h| app.hs.get(h).is_some_and(|s| s.wrapped))
                .filter(|h| app.derived.bins.get(h).is_some_and(|b| b.is_some()))
                .map(|h| h.spec().bins[0])
                .collect();
            vec![
                note(
                    "Shell",
                    shell_name,
                    "Read from $SHELL.",
                ),
                note(
                    "Integration",
                    state,
                    "With it loaded, typing the agent's name goes through cswap. Without it, `cswap claude` and friends still work.",
                ),
                note(
                    "Line for your rc file",
                    init,
                    "Add this line to your shell's rc file, then open a new terminal.",
                ),
                note(
                    "Commands it wraps",
                    if wrapped.is_empty() { "none".to_string() } else { wrapped.join(", ") },
                    "Each is a small function that runs `cswap <agent>`. Turn wrapping on or off per agent on its tab.",
                ),
                note(
                    "Pick an account once",
                    "claude --cswap=work".to_string(),
                    "--cswap=<name or number> picks the account for one launch. --cswap=auto takes the best one, --cswap=default the agent's default login. --cswap-select asks.",
                ),
                note(
                    "Pin a terminal",
                    "export CSWAP_CLAUDE=work".to_string(),
                    "CSWAP_<AGENT> pins every launch from one terminal. A wrapped session sets it for its own children, so they stay on its account.",
                ),
            ]
        }
    }
}

fn step(options: &[u64], current: u64, dir: i32) -> u64 {
    if dir < 0 {
        options
            .iter()
            .rev()
            .copied()
            .find(|v| *v < current)
            .unwrap_or(options[0])
    } else {
        options
            .iter()
            .copied()
            .find(|v| *v > current)
            .unwrap_or(options[options.len() - 1])
    }
}

fn ask(app: &mut App, id: Id, title: &str, prompt: &str, current: String, hint: &str) {
    app.modal = Some(Modal::Input {
        title: title.to_string(),
        prompt: prompt.to_string(),
        value: current,
        hint: hint.to_string(),
        error: None,
        then: Then::Setting(id),
    });
}

/// Change the selected row: -1 and +1 step it, 0 toggles it or opens an editor.
pub fn change(app: &mut App, dir: i32) {
    let Some(row) = rows(app).into_iter().nth(app.row()) else {
        return;
    };
    let Some(id) = row.id else {
        return;
    };
    let flip_h =
        |h: Harness, f: fn(&mut HarnessSettings)| HarnessSettings::update(h, f).map(|_| ());
    let result: Result<()> = match id {
        Id::RefreshMinutes if dir == 0 => {
            let current = app.settings.refresh_minutes.to_string();
            ask(
                app,
                id,
                "Refresh every",
                "Minutes",
                current,
                &format!("a whole number, {MIN_REFRESH_MINUTES} or more"),
            );
            return;
        }
        Id::RefreshMinutes => Settings::update(|s| {
            s.refresh_minutes = step(&[2, 5, 10, 15, 20, 30, 60, 120], s.refresh_minutes, dir)
        })
        .map(|_| ()),
        Id::AutoSkip => Settings::update(|s| {
            let d = if dir < 0 { -1.0 } else { 1.0 };
            s.auto_skip_above_pct = (s.auto_skip_above_pct + d).clamp(50.0, 100.0)
        })
        .map(|_| ()),
        Id::HistoryDays => Settings::update(|s| {
            s.history_days = step(&[14, 30, 60, 90, 180, 365], s.history_days as u64, dir) as u32
        })
        .map(|_| ()),
        Id::RefreshOnOpen => {
            Settings::update(|s| s.refresh_on_open = !s.refresh_on_open).map(|_| ())
        }
        Id::Clock24h => Settings::update(|s| s.clock_24h = !s.clock_24h).map(|_| ()),
        Id::Mouse => Settings::update(|s| s.mouse = !s.mouse).map(|_| ()),
        Id::PaintBackground => {
            Settings::update(|s| s.paint_background = !s.paint_background).map(|_| ())
        }
        Id::Theme => Settings::update(|s| {
            s.theme = crate::tui::theme::Scheme::parse(&s.theme)
                .step(dir)
                .name()
                .to_string()
        })
        .map(|_| ()),
        Id::UpdateCheck => Settings::update(|s| s.update_check = !s.update_check).map(|_| ()),
        Id::Wrapped(h) => flip_h(h, |s| s.wrapped = !s.wrapped),
        Id::Announce(h) => flip_h(h, |s| s.announce = !s.announce),
        Id::SyncEnabled(h) => flip_h(h, |s| s.sync.enabled = !s.sync.enabled),
        Id::CliIdentity(h) => flip_h(h, |s| s.cli_identity = !s.cli_identity),
        Id::Default(h) => {
            let mut options = vec!["default".to_string(), "auto".to_string()];
            options.extend(
                app.reg
                    .of(h)
                    .filter(|a| !a.is_native())
                    .map(|a| a.meta.id.to_string()),
            );
            let current = app.hs.get(&h).map(|s| s.start.clone()).unwrap_or_default();
            let at = options.iter().position(|o| *o == current).unwrap_or(0) as i64;
            let n = options.len() as i64;
            let next = (at + if dir < 0 { -1 } else { 1 }).rem_euclid(n) as usize;
            manage::set_start(h, &options[next]).map(|_| ())
        }
        Id::Exclude(h) => {
            if dir != 0 {
                return;
            }
            let current = app
                .hs
                .get(&h)
                .map(|s| s.sync.exclude.join(", "))
                .unwrap_or_default();
            ask(
                app,
                id,
                &format!("Extra private names for {}", h.name()),
                "Names",
                current,
                "separate with commas · * matches at either end · empty clears the list",
            );
            return;
        }
        Id::Binary(h) => {
            if dir != 0 {
                return;
            }
            let current = app
                .hs
                .get(&h)
                .and_then(|s| s.binary.clone())
                .unwrap_or_default();
            ask(
                app,
                id,
                &format!("Program path for {}", h.name()),
                "Path",
                current,
                "a full path · empty goes back to searching PATH",
            );
            return;
        }
    };
    match result {
        Ok(()) => app.reload(),
        Err(e) => app.say(Tone::Bad, format!("{e:#}")),
    }
}

/// Store the text typed for a setting. An error is shown in the dialog.
pub fn set_text(_app: &mut App, id: Id, text: &str) -> Result<(), String> {
    let text = text.trim();
    let saved = match id {
        Id::RefreshMinutes => {
            let n: u64 = text
                .parse()
                .map_err(|_| "type a whole number of minutes".to_string())?;
            if !(MIN_REFRESH_MINUTES..=1440).contains(&n) {
                return Err(format!("between {MIN_REFRESH_MINUTES} and 1440 minutes"));
            }
            Settings::update(|s| s.refresh_minutes = n).map(|_| ())
        }
        Id::Exclude(h) => {
            let names: Vec<String> = text
                .split([',', ' '])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if let Some(bad) = names.iter().find(|n| n.contains('/')) {
                return Err(format!("'{bad}': a name, not a path"));
            }
            HarnessSettings::update(h, |s| s.sync.exclude = names).map(|_| ())
        }
        Id::Binary(h) => {
            if text.is_empty() {
                HarnessSettings::update(h, |s| s.binary = None).map(|_| ())
            } else {
                let path = std::path::Path::new(text);
                if !path.is_absolute() {
                    return Err("give the full path, starting from /".to_string());
                }
                if !path.is_file() {
                    return Err("no file at that path".to_string());
                }
                let text = text.to_string();
                HarnessSettings::update(h, |s| s.binary = Some(text)).map(|_| ())
            }
        }
        _ => return Err("this setting is not typed in".to_string()),
    };
    saved.map_err(|e| format!("{e:#}"))
}

pub fn draw(f: &mut Frame, area: Rect, app: &App, hits: &mut Hits) {
    let t = &app.theme;
    if area.height < 5 {
        return;
    }
    // The tab strip.
    let mut x = area.x + 1;
    for (i, tab) in TABS.iter().enumerate() {
        let label = format!(" {} ", tab.title());
        let w = width(&label) as u16;
        if x + w > area.x + area.width {
            break;
        }
        let rect = Rect::new(x, area.y, w, 1);
        let current = i == app.settings_tab;
        let color = match tab {
            Tab::Agent(h) => t.harness(*h),
            _ => t.accent(),
        };
        let style = if current {
            Style::default()
                .fg(t.on_accent())
                .bg(color)
                .add_modifier(Modifier::BOLD)
        } else {
            t.s_dim()
        };
        f.render_widget(Paragraph::new(Span::styled(label, style)), rect);
        hits.add(rect, Action::SettingsTab(i));
        x += w + 1;
    }

    let body = Rect::new(area.x, area.y + 2, area.width, area.height - 2);
    let focused = app.focus == Focus::Main && app.page == Page::Settings;
    let inner = card(f, body, "", t, None);
    let rows = rows(app);
    let help_h: u16 = 4;
    let list_h = inner.height.saturating_sub(help_h) as usize;
    let sel = app.row().min(rows.len().saturating_sub(1));
    let start = scroll_start(sel, rows.len(), list_h);
    let w = inner.width as usize;
    let label_w = (w / 2).clamp(18, 32);

    for (i, row) in rows.iter().enumerate().skip(start).take(list_h) {
        let rect = Rect::new(inner.x, inner.y + (i - start) as u16, inner.width, 1);
        let selected = focused && i == sel;
        let (mark, bg) = row_style(app, selected);
        if selected {
            paint_row(f, rect, bg);
        }
        let editable = row.id.is_some();
        let label_style = if selected {
            t.s_bold()
        } else if editable {
            t.s_text()
        } else {
            t.s_dim()
        };
        let room = w.saturating_sub(label_w + 2);
        let (text, style) = match row.on {
            Some(true) => (format!("{DOT} on "), t.s(t.ok())),
            Some(false) => (format!("{RING} off"), t.s_faint()),
            None if editable && selected => (
                format!(
                    "‹ {} ›",
                    crate::tui::widgets::truncate(&row.value, room.saturating_sub(4))
                ),
                t.s_accent(),
            ),
            None if editable => (
                format!(
                    "{}  ",
                    crate::tui::widgets::truncate(&row.value, room.saturating_sub(2))
                ),
                t.s_text(),
            ),
            None => (
                format!(
                    "{}  ",
                    crate::tui::widgets::truncate(&row.value, room.saturating_sub(2))
                ),
                t.s_dim(),
            ),
        };
        let text_w = width(&text);
        let gap = w.saturating_sub(1 + label_w + text_w + 1);
        let line = Line::from(vec![
            mark,
            Span::styled(fit(&row.label, label_w), label_style),
            Span::raw(" ".repeat(gap)),
            Span::styled(text, style),
        ]);
        f.render_widget(Paragraph::new(line), rect);
        hits.add(rect, Action::Row(i));
        // The arrows of the selected value step it.
        if selected && row.on.is_none() && editable {
            let end = inner.x + inner.width - 1;
            let left = end.saturating_sub(text_w as u16);
            hits.add(Rect::new(left, rect.y, 2, 1), Action::SettingChange(-1));
            hits.add(
                Rect::new(end.saturating_sub(2), rect.y, 2, 1),
                Action::SettingChange(1),
            );
        }
    }

    // What the selected row means.
    if let Some(row) = rows.get(sel) {
        let y = inner.y + inner.height.saturating_sub(help_h);
        f.render_widget(
            Paragraph::new(Span::styled("─".repeat(w), t.s(t.border()))),
            Rect::new(inner.x, y, inner.width, 1),
        );
        f.render_widget(
            Paragraph::new(Span::styled(row.help.clone(), t.s_dim())).wrap(Wrap { trim: true }),
            Rect::new(
                inner.x + 1,
                y + 1,
                inner.width.saturating_sub(2),
                help_h - 1,
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_moves_to_the_next_option_and_stops_at_the_ends() {
        let o = [2, 5, 10, 15];
        assert_eq!(step(&o, 10, 1), 15);
        assert_eq!(step(&o, 15, 1), 15);
        assert_eq!(step(&o, 10, -1), 5);
        assert_eq!(step(&o, 2, -1), 2);
        // A value typed by hand that is not an option still steps sensibly.
        assert_eq!(step(&o, 7, 1), 10);
        assert_eq!(step(&o, 7, -1), 5);
    }
}
