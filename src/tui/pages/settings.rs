//! Settings, in three tabs: the dashboard's own options, the options of
//! every agent in one list, and the shells with the agents each one wraps.

use anyhow::Result;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::{head, paint_row, row_style, scroll_start};
use crate::harness::Harness;
use crate::manage;
use crate::paths;
use crate::shell;
use crate::store::{HarnessSettings, Settings, MIN_REFRESH_MINUTES};
use crate::tui::app::{Action, App, Focus, Modal, Page, Then, Tone, UpdateCheck};
use crate::tui::theme::{DOT, RING};
use crate::tui::widgets::{self, card, clip, fit, truncate, width, Hits};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    General,
    Harness,
    Shell,
}

pub const TABS: [Tab; 3] = [Tab::General, Tab::Harness, Tab::Shell];

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Harness => "Harness",
            Tab::Shell => "Shell",
        }
    }
}

/// One thing that can be changed or done.
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
    /// Ask GitHub now whether a newer cswap is out.
    CheckNow,
    AutoPick(Harness),
    Announce(Harness),
    SyncEnabled(Harness),
    Exclude(Harness),
    Binary(Harness),
    CliIdentity(Harness),
    /// The agents a shell wraps: an index into the shells found.
    Shell(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A line that only informs.
    Note,
    Toggle(bool),
    /// A value that steps, or is typed in.
    Value,
    /// Something to do: enter does it.
    Button,
    /// One shell of the table: an index into the shells found.
    Shell(usize),
}

pub struct Row {
    /// `None` for a line that only informs.
    pub id: Option<Id>,
    pub label: String,
    pub value: String,
    pub help: String,
    pub kind: Kind,
    /// The title of the group this row starts, and the agent it is about.
    pub group: Option<(String, Option<Harness>)>,
}

impl Row {
    fn new(id: Option<Id>, kind: Kind, label: &str, value: String, help: &str) -> Row {
        Row {
            id,
            label: label.to_string(),
            value,
            help: help.to_string(),
            kind,
            group: None,
        }
    }

    /// Start a group at this row.
    fn under(mut self, title: &str, agent: Option<Harness>) -> Row {
        self.group = Some((title.to_string(), agent));
        self
    }
}

fn toggle(id: Id, label: &str, on: bool, help: &str) -> Row {
    let value = if on { "on" } else { "off" }.to_string();
    Row::new(Some(id), Kind::Toggle(on), label, value, help)
}

fn value(id: Id, label: &str, value: String, help: &str) -> Row {
    Row::new(Some(id), Kind::Value, label, value, help)
}

fn button(id: Id, label: &str, value: String, help: &str) -> Row {
    Row::new(Some(id), Kind::Button, label, value, help)
}

fn note(label: &str, value: String, help: &str) -> Row {
    Row::new(None, Kind::Note, label, value, help)
}

/// Where the check for a newer cswap stands, in a few words.
fn update_text(app: &App) -> String {
    let have = crate::update::CURRENT;
    match &app.update {
        UpdateCheck::Checking => "checking…".to_string(),
        UpdateCheck::Newest => format!("{have} is the newest"),
        UpdateCheck::Newer(v) => format!("{v} is out · you have {have}"),
        UpdateCheck::Failed(_) => format!("could not check · you have {have}"),
        UpdateCheck::NotAsked => match &app.derived.newer {
            Some(v) => format!("{v} is out · you have {have}"),
            None => format!("you have {have}"),
        },
    }
}

fn general(app: &App) -> Vec<Row> {
    let s = &app.settings;
    vec![
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
            "How often usage is fetched while this dashboard is open. A launch through cswap never fetches: it reads what the dashboard last stored.",
        ),
        toggle(
            Id::RefreshOnOpen,
            "Refresh when opened",
            s.refresh_on_open,
            "Fetch whatever is older than the interval as soon as the dashboard opens.",
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
            "Check once a day",
            s.update_check,
            "Looks at GitHub at most once a day and tells you on the command line. It never updates on its own: `cswap update` does that.",
        )
        .under("Updates", None),
        button(
            Id::CheckNow,
            "Check for updates now",
            update_text(app),
            "Asks GitHub for the newest release right now and says what it found. Installing it is still `cswap update`.",
        ),
    ]
}

/// Every agent's options in one list, a group per agent, after the options
/// they all share.
fn harness(app: &App) -> Vec<Row> {
    let mut out = vec![value(
        Id::AutoSkip,
        "Auto-pick skips above",
        format!("{:.0}%", app.settings.auto_skip_above_pct),
        "Auto-pick and the suggestions leave out an account whose week is this full.",
    )
    .under("All agents", None)];
    for h in Harness::ALL {
        let hs = app.hs.get(&h).cloned().unwrap_or_default();
        let spec = h.spec();
        let bin = spec.bins[0];
        let program = match (
            &hs.binary,
            app.derived.bins.get(&h).and_then(|b| b.as_ref()),
        ) {
            (Some(set), _) => format!("{set} · set here"),
            (None, Some(found)) => paths::tilde(found),
            (None, None) => "not installed".to_string(),
        };
        out.push(
            value(
                Id::Binary(h),
                "Program",
                program,
                "The real program cswap starts: the first one on your PATH. Enter sets a full path instead; an empty one goes back to searching PATH.",
            )
            .under(h.name(), Some(h)),
        );
        out.push(note(
            "Its own config dir",
            paths::tilde(&h.native_dir()),
            "Where the agent keeps its state. cswap reads it and links to it, and never writes into it.",
        ));
        out.push(toggle(
            Id::AutoPick(h),
            "Auto-pick the account",
            hs.start == "auto",
            &format!("On: `cswap {bin}` with no account named takes the best account at every launch, so no account is the default. Off: it starts on the default. Set the default with d on the Accounts page, or on the Root page for the agent's own login."),
        ));
        out.push(toggle(
            Id::Announce(h),
            "Say which account at launch",
            hs.announce,
            "One dim line on stderr before the agent starts, and only when cswap chose something other than the agent's root login.",
        ));
        out.push(toggle(
            Id::SyncEnabled(h),
            "Sync links at launch",
            hs.sync.enabled,
            "Bring each account's links up to date with the agent's own config dir every time it starts, so new files there are shared at once.",
        ));
        out.push(value(
            Id::Exclude(h),
            "Extra private names",
            if hs.sync.exclude.is_empty() {
                "none".to_string()
            } else {
                hs.sync.exclude.join(", ")
            },
            "Names every account keeps to itself, on top of the built-in list. Separate with commas; * matches at either end.",
        ));
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
                "cswap accounts for this agent rely on behaviour its vendor does not document. The agent's root login always works.",
            ));
        }
    }
    out
}

/// What a shell's row means, and what enter does to it.
fn shell_help(sh: &shell::Status) -> String {
    match (&sh.rc, sh.loaded) {
        (Some(rc), Some(true)) => format!(
            "{} loads cswap. An integrated agent runs through cswap when you type its name in {}, so it starts on the account cswap picks. Enter chooses the agents.",
            paths::tilde(rc),
            sh.name
        ),
        (Some(rc), _) => format!(
            "{} does not load cswap, so {} wraps no agent yet. Enter chooses the agents; saving adds one line to that file: {}",
            paths::tilde(rc),
            sh.name,
            shell::line(sh.name)
        ),
        (None, _) => format!(
            "cswap cannot read {}'s profile, so this is what its list holds. Enter chooses the agents; the line to add to the profile yourself is: {}",
            sh.name,
            shell::line(sh.name)
        ),
    }
}

fn shells(app: &App) -> Vec<Row> {
    let mut out: Vec<Row> = app
        .derived
        .shells
        .iter()
        .enumerate()
        .map(|(i, sh)| {
            Row::new(
                Some(Id::Shell(i)),
                Kind::Shell(i),
                sh.name,
                String::new(),
                &shell_help(sh),
            )
        })
        .collect();
    if out.is_empty() {
        out.push(note(
            "Shells",
            "none found".to_string(),
            "cswap wraps agents in zsh, bash, fish and PowerShell, and none of them is on your PATH. `cswap claude` and friends still work.",
        ));
    }
    out[0].group = Some(("Integration".to_string(), None));
    out.push(
        note(
            "Pick an account once",
            "claude --cswap=work".to_string(),
            "--cswap=<name or number> picks the account for one launch. --cswap=auto takes the best one, --cswap=root the agent's root login. --cswap-select asks.",
        )
        .under("In a terminal", None),
    );
    out.push(note(
        "Pin a terminal",
        "export CSWAP_CLAUDE=work".to_string(),
        "CSWAP_<AGENT> pins every launch from one terminal. A session started through cswap sets it for its own children, so they stay on its account.",
    ));
    out.push(note(
        "Without integration",
        "cswap claude".to_string(),
        "`cswap <agent>` always goes through cswap, in any shell, whether or not the agent is integrated there.",
    ));
    out
}

pub fn rows(app: &App) -> Vec<Row> {
    match TABS[app.settings_tab.min(TABS.len() - 1)] {
        Tab::General => general(app),
        Tab::Harness => harness(app),
        Tab::Shell => shells(app),
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

/// Change the selected row: -1 and +1 step it, 0 toggles it, opens its
/// editor or does what it says.
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
        Id::CheckNow => {
            if dir == 0 {
                app.check_update();
            }
            return;
        }
        Id::Shell(at) => {
            if dir == 0 {
                app.shell_menu(at);
            }
            return;
        }
        Id::Announce(h) => flip_h(h, |s| s.announce = !s.announce),
        Id::SyncEnabled(h) => flip_h(h, |s| s.sync.enabled = !s.sync.enabled),
        Id::CliIdentity(h) => flip_h(h, |s| s.cli_identity = !s.cli_identity),
        // Off goes back to the agent's root login: the account that was the
        // default before auto-pick is not remembered.
        Id::AutoPick(h) => {
            let on = app.hs.get(&h).is_some_and(|s| s.start == "auto");
            manage::set_start(h, if on { "root" } else { "auto" }).map(|_| ())
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

/// One line of the list as it is drawn: the rows, with the titles of their
/// groups and the header of the shell table between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vis {
    Gap,
    /// The title of the group the row at this index starts.
    Title(usize),
    /// The header of the shell table.
    Head,
    Row(usize),
}

fn layout(rows: &[Row]) -> Vec<Vis> {
    let is_shell = |r: &Row| matches!(r.kind, Kind::Shell(_));
    let mut out = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if row.group.is_some() {
            if !out.is_empty() {
                out.push(Vis::Gap);
            }
            out.push(Vis::Title(i));
        }
        if is_shell(row) && !(i > 0 && is_shell(&rows[i - 1])) {
            out.push(Vis::Head);
        }
        out.push(Vis::Row(i));
    }
    out
}

/// The width of the shell's name in the shell table.
const SHELL_W: usize = 14;

/// Spans cut or padded to exactly `w` cells, the last of them blank.
fn cell(spans: Vec<Span<'static>>, w: usize) -> Vec<Span<'static>> {
    let mut out = clip(spans, w.saturating_sub(1));
    let used = widgets::line_width(&Line::from(out.clone()));
    out.push(Span::raw(" ".repeat(w.saturating_sub(used))));
    out
}

/// One shell: its name, the agents that run through cswap in it, and the
/// installed agents that do not.
fn shell_cells(
    app: &App,
    sh: &shell::Status,
    selected: bool,
    on_w: usize,
    off_w: usize,
) -> Vec<Span<'static>> {
    let t = &app.theme;
    let mut name = vec![Span::styled(
        sh.name.to_string(),
        if selected { t.s_bold() } else { t.s_text() },
    )];
    if sh.current {
        name.push(Span::styled("  yours", t.s_faint()));
    }
    let agents = |on: bool| -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for h in Harness::ALL {
            let installed = app.derived.bins.get(&h).is_some_and(|b| b.is_some());
            if !installed || sh.wraps(h) != on {
                continue;
            }
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                h.spec().bins[0].to_string(),
                if on { t.s(t.harness(h)) } else { t.s_dim() },
            ));
        }
        if spans.is_empty() {
            spans.push(Span::styled("–", t.s_faint()));
        }
        spans
    };
    let mut out = cell(name, SHELL_W);
    out.extend(cell(agents(true), on_w));
    out.extend(cell(agents(false), off_w));
    out
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
        let style = if i == app.settings_tab {
            Style::default()
                .fg(t.on_accent())
                .bg(t.accent())
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
    let lines = layout(&rows);
    let help_h: u16 = 5;
    let list_h = inner.height.saturating_sub(help_h) as usize;
    let sel = app.row().min(rows.len().saturating_sub(1));
    let sel_line = lines.iter().position(|l| *l == Vis::Row(sel)).unwrap_or(0);
    let start = scroll_start(sel_line, lines.len(), list_h);
    let w = inner.width as usize;
    let label_w = (w / 2).clamp(18, 32);
    // The shell table: the name, then two columns that share the rest.
    let on_w = w.saturating_sub(1 + SHELL_W) / 2;
    let off_w = w.saturating_sub(1 + SHELL_W) - on_w;

    for (n, line) in lines.iter().enumerate().skip(start).take(list_h) {
        let rect = Rect::new(inner.x, inner.y + (n - start) as u16, inner.width, 1);
        let i = match *line {
            Vis::Gap => continue,
            Vis::Title(i) => {
                let Some((title, agent)) = &rows[i].group else {
                    continue;
                };
                let (dot, color) = match agent {
                    Some(h) => (
                        Span::styled(format!(" {DOT} "), Style::default().fg(t.harness(*h))),
                        t.dim(),
                    ),
                    None => (Span::raw(" "), t.faint()),
                };
                let title = Span::styled(
                    title.to_uppercase(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                );
                f.render_widget(Paragraph::new(Line::from(vec![dot, title])), rect);
                continue;
            }
            Vis::Head => {
                let line = Line::from(vec![
                    Span::raw(" "),
                    head(t, "SHELL", SHELL_W),
                    head(t, "INTEGRATED", on_w),
                    head(t, "NOT INTEGRATED", off_w),
                ]);
                f.render_widget(Paragraph::new(line), rect);
                continue;
            }
            Vis::Row(i) => i,
        };
        let row = &rows[i];
        let selected = focused && i == sel;
        let (mark, bg) = row_style(app, selected);
        if selected {
            paint_row(f, rect, bg);
        }
        hits.add(rect, Action::Row(i));
        if let Kind::Shell(at) = row.kind {
            let Some(sh) = app.derived.shells.get(at) else {
                continue;
            };
            let mut spans = vec![mark];
            spans.extend(shell_cells(app, sh, selected, on_w, off_w));
            f.render_widget(Paragraph::new(Line::from(spans)), rect);
            continue;
        }
        let label_style = if selected {
            t.s_bold()
        } else if row.id.is_some() {
            t.s_text()
        } else {
            t.s_dim()
        };
        let room = w.saturating_sub(label_w + 2);
        let (text, style) = match row.kind {
            Kind::Toggle(true) => (format!("{DOT} on "), t.s(t.ok())),
            Kind::Toggle(false) => (format!("{RING} off"), t.s_faint()),
            Kind::Value if selected => (
                format!("‹ {} ›", truncate(&row.value, room.saturating_sub(4))),
                t.s_accent(),
            ),
            Kind::Button if selected => (
                format!("{} ⏎", truncate(&row.value, room.saturating_sub(2))),
                t.s_accent(),
            ),
            Kind::Value | Kind::Button => (
                format!("{}  ", truncate(&row.value, room.saturating_sub(2))),
                t.s_text(),
            ),
            Kind::Note | Kind::Shell(_) => (
                format!("{}  ", truncate(&row.value, room.saturating_sub(2))),
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
        // The arrows of the selected value step it.
        if selected && row.kind == Kind::Value {
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

    #[test]
    fn a_group_gets_a_title_and_the_shells_one_header() {
        let row = |kind: Kind, group: bool| {
            let r = Row::new(None, kind, "x", String::new(), "");
            if group {
                r.under("G", None)
            } else {
                r
            }
        };
        let rows = vec![
            row(Kind::Shell(0), true),
            row(Kind::Shell(1), false),
            row(Kind::Note, true),
            row(Kind::Note, false),
        ];
        assert_eq!(
            layout(&rows),
            vec![
                Vis::Title(0),
                Vis::Head,
                Vis::Row(0),
                Vis::Row(1),
                Vis::Gap,
                Vis::Title(2),
                Vis::Row(2),
                Vis::Row(3),
            ]
        );
    }
}
