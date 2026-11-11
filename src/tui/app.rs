//! The dashboard's state, and everything a key press or a click can do to it.
//!
//! Disk is the source of truth. Every action writes through `store` or
//! `manage` and then reloads, so the dashboard, the command line and a wrapped
//! launch can never disagree for longer than one reload.

use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use super::pages;
use super::theme::Theme;
use super::widgets::Hits;
use crate::harness::{Harness, Local};
use crate::health::{self, Check};
use crate::history::{self, Sample};
use crate::model::{Grant, Source};
use crate::sessions::{self, Live};
use crate::store::{Account, Cycle, HarnessSettings, Registry, Settings};
use crate::suggest::{self, Pick};
use crate::sync::{self, Report};
use crate::{manage, paths, refresh, store};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    Overview,
    Usage,
    Resets,
    Accounts,
    Root,
    Sync,
    Sessions,
    Analytics,
    Health,
    Integrations,
    Settings,
    Help,
}

impl Page {
    /// In the order of the menu, which is also the order of the number keys.
    pub const ALL: [Page; 12] = [
        Page::Overview,
        Page::Usage,
        Page::Resets,
        Page::Accounts,
        Page::Root,
        Page::Sync,
        Page::Sessions,
        Page::Analytics,
        Page::Health,
        Page::Integrations,
        Page::Settings,
        Page::Help,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Overview => "Overview",
            Page::Usage => "Usage",
            Page::Accounts => "Accounts",
            Page::Root => "Root",
            Page::Sessions => "Sessions",
            Page::Resets => "Resets",
            Page::Sync => "Sync",
            Page::Analytics => "Analytics",
            Page::Health => "Health",
            Page::Integrations => "Integrations",
            Page::Settings => "Settings",
            Page::Help => "Help",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Page::Overview => "what to use next, and every account at a glance",
            Page::Usage => "how much of each limit is used, and when it resets",
            Page::Accounts => "add, sign in, name and remove accounts",
            Page::Root => "each agent's own login, the one it had before cswap",
            Page::Sessions => "the agents running right now",
            Page::Resets => "what resets when, and the resets you hold · view only",
            Page::Sync => "what each account shares with the agent's own config dir",
            Page::Analytics => "how fast each account burns through its week",
            Page::Health => "checks for the problems that are otherwise silent",
            Page::Integrations => {
                "cswap inside other programs · fetched from GitHub when you install one"
            }
            Page::Settings => "the dashboard, every agent's options, and the shells that wrap them",
            Page::Help => "starting an agent on an account, cswap's words, and every key",
        }
    }

    /// The key that jumps to this page: 1 to 9 down the menu, then `i` for
    /// Integrations, 0 for Settings and `?` for Help.
    pub fn hotkey(self) -> char {
        match self {
            Page::Help => '?',
            Page::Integrations => 'i',
            Page::Settings => '0',
            _ => char::from_digit(self.index() as u32 + 1, 10).unwrap_or('1'),
        }
    }

    pub fn index(self) -> usize {
        Page::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Nav,
    Main,
}

/// Everything a key or a click can ask for.
#[derive(Debug, Clone)]
pub enum Action {
    Go(Page),
    /// Select a row of the current page; a second click on it opens it.
    Row(usize),
    Activate,
    RefreshAll,
    Refresh(String),
    AddAccount,
    /// Ask for the new account's name; the text is a suggestion.
    NewAccountName(Harness, String),
    /// Make an account for the login an agent's root holds.
    NewFromRoot(String),
    AccountMenu(String),
    Login(String),
    DoLogin(String),
    Remove(String),
    DoRemove(String),
    EditLabel(String),
    EditAliases(String),
    EditPlan(String),
    EditBilling(String),
    BillingCycle(String, NaiveDate, Option<Cycle>),
    BillingSave(String, NaiveDate, Option<Cycle>, bool),
    AddGrant(String),
    ClearGrants(String),
    SetDefault(String),
    Launch(String),
    SyncAll,
    SyncOne(String),
    Repair(String),
    DoRepair(String),
    SettingsTab(usize),
    /// -1 and +1 step a value; 0 activates the row (toggle or edit).
    SettingChange(i32),
    ModalPick(usize),
    /// Tick or clear one box of a list of ticks.
    Tick(usize),
    /// A click that lands on a dialog and on nothing in it.
    Ignore,
    ModalOk,
    ModalCancel,
    Help,
    /// What can be done with the selected integration.
    IntegrationMenu,
    /// Fetch the herdr layout and put it into herdr's folder.
    InstallHerdr,
    /// Ask the running herdr to show the cswap layout.
    ShowHerdr,
    /// One step back: from the page to the menu, from the menu out.
    Back,
}

/// Menu items in groups: the first item of each group carries its title.
fn grouped(groups: Vec<(&'static str, Vec<ChoiceItem>)>) -> Vec<ChoiceItem> {
    groups
        .into_iter()
        .flat_map(|(heading, mut items)| {
            if let Some(first) = items.first_mut() {
                first.heading = Some(heading);
            }
            items
        })
        .collect()
}

/// What "Set as default" means, said beside it in a menu.
fn default_detail(bin: &str, is_default: bool) -> String {
    if is_default {
        "it already is".to_string()
    } else {
        format!("what `cswap {bin}` starts on")
    }
}

/// What to do with the text an input modal collected.
#[derive(Debug, Clone)]
pub enum Then {
    CreateAccount(Harness),
    Label(String),
    Aliases(String),
    Plan(String),
    BillingDate(String),
    GrantExpiry(String),
    Setting(pages::settings::Id),
}

#[derive(Debug, Clone)]
pub struct ChoiceItem {
    /// The title of the group this item starts. An empty one only sets the
    /// item apart from the group above it.
    pub heading: Option<&'static str>,
    pub label: String,
    pub detail: String,
    pub enabled: bool,
    pub key: Option<char>,
    pub action: Action,
}

/// One box in a list of ticks.
#[derive(Debug, Clone)]
pub struct TickItem {
    /// What the tick stands for, such as an agent's id.
    pub id: String,
    pub label: String,
    pub detail: String,
    pub enabled: bool,
    /// Ticked when the dialog opened: what is saved now.
    pub was: bool,
    pub on: bool,
}

/// What a list of ticks is for.
#[derive(Debug, Clone)]
pub enum Ticks {
    /// The agents a shell wraps.
    Shell(String),
}

#[derive(Debug, Clone)]
pub enum Modal {
    Choice {
        title: String,
        hint: String,
        items: Vec<ChoiceItem>,
        sel: usize,
    },
    /// Boxes to tick. Nothing changes until it is saved.
    Ticks {
        title: String,
        hint: String,
        items: Vec<TickItem>,
        sel: usize,
        then: Ticks,
    },
    Input {
        title: String,
        prompt: String,
        value: String,
        hint: String,
        error: Option<String>,
        then: Then,
    },
    Confirm {
        title: String,
        body: Vec<String>,
        yes: String,
        danger: bool,
        action: Box<Action>,
    },
    Info {
        title: String,
        body: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Good,
    Warn,
    Bad,
}

pub struct Toast {
    pub text: String,
    pub tone: Tone,
    pub at: Instant,
}

pub enum Msg {
    Refreshed {
        key: String,
        error: Option<String>,
    },
    /// The answer to a check asked for in Settings: a newer version, or none.
    UpdateChecked(Result<Option<String>, String>),
    /// How installing the herdr layout went.
    Integrated(Result<crate::integration::Installed, String>),
}

/// Where the check for a newer cswap asked for in Settings stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdateCheck {
    #[default]
    NotAsked,
    Checking,
    Newest,
    Newer(String),
    Failed(String),
}

/// Work that needs the real terminal back: the agent's own sign-in.
pub enum Suspend {
    Login { key: String, created: bool },
}

/// What to do after the dashboard closes.
pub enum Exit {
    Launch { key: String },
}

/// One running agent, for the Sessions page.
#[derive(Debug, Clone)]
pub struct Session {
    pub key: String,
    pub pid: u32,
    pub cwd: Option<PathBuf>,
    pub started: Option<chrono::DateTime<Utc>>,
}

/// Facts read from disk on every reload, so drawing a frame touches no files.
#[derive(Default)]
pub struct Derived {
    pub local: HashMap<String, Local>,
    pub logged_in: HashSet<String>,
    pub picks: HashMap<Harness, Vec<Pick>>,
    pub bins: HashMap<Harness, Option<PathBuf>>,
    pub checks: Vec<Check>,
    /// A newer cswap release, when the last check found one.
    pub newer: Option<String>,
    /// The shells on this machine and the agents each one wraps.
    pub shells: Vec<crate::shell::Status>,
}

pub struct App {
    pub settings: Settings,
    pub hs: HashMap<Harness, HarnessSettings>,
    /// The accounts cswap holds. Every list of accounts shows these.
    pub reg: Registry,
    /// The same, plus each agent's own login. Only the Root page and the
    /// work behind the scenes (refresh, health, sessions) look here.
    pub all: Registry,
    pub live: Live,
    pub sessions: Vec<Session>,
    pub derived: Derived,
    pub theme: Theme,

    pub page: Page,
    pub focus: Focus,
    /// Selected row per page.
    pub sel: [usize; 12],
    /// The page `?` was pressed on, to go back to with a second `?`.
    pub before_help: Page,
    /// How wide and how tall the Help page's text was when last drawn, so
    /// the keys scroll it no further than its end.
    pub help_view: (usize, usize),
    pub settings_tab: usize,
    /// Where the herdr integration stands, read when its page is opened.
    pub integration: crate::integration::State,
    /// An install is running on its thread.
    pub installing: bool,
    pub update: UpdateCheck,
    pub modal: Option<Modal>,
    pub toast: Option<Toast>,
    pub hits: Hits,

    pub busy: HashSet<String>,
    pub sync_reports: HashMap<String, Report>,
    pub history: HashMap<String, Vec<Sample>>,

    pub suspend: Option<Suspend>,
    pub exit: Option<Exit>,
    pub quit: bool,

    tx: Sender<Msg>,
    workers: Vec<std::thread::JoinHandle<()>>,
    last_reload: Instant,
    last_scan: Instant,
    last_due_check: Instant,
    opened: Instant,
}

/// `2026-12-01`, or `30d` for thirty days from today. Empty is "none".
pub fn parse_date(text: &str) -> Result<Option<NaiveDate>, String> {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return Ok(None);
    }
    if let Some(days) = t
        .strip_suffix('d')
        .and_then(|n| n.trim_start_matches('+').parse::<i64>().ok())
    {
        return Ok(Some(store::today() + ChronoDuration::days(days)));
    }
    NaiveDate::parse_from_str(&t, "%Y-%m-%d")
        .map(Some)
        .map_err(|_| "use YYYY-MM-DD, or a number of days such as 30d".to_string())
}

/// The end of a local day as a UTC time, for an expiry given as a date.
fn end_of_day(date: NaiveDate) -> DateTime<Utc> {
    use chrono::TimeZone;
    let naive = date.and_hms_opt(23, 59, 59).expect("valid time");
    chrono::Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|| naive.and_utc())
}

impl App {
    pub fn new(tx: Sender<Msg>) -> Result<App> {
        let settings = Settings::load()?;
        let now = Instant::now();
        let mut app = App {
            theme: Theme::detect(settings.paint_background, &settings.theme),
            settings,
            hs: HashMap::new(),
            reg: Registry::default(),
            all: Registry::default(),
            live: Live::new(),
            sessions: Vec::new(),
            derived: Derived::default(),
            page: Page::Overview,
            focus: Focus::Nav,
            sel: [0; 12],
            before_help: Page::Overview,
            help_view: (0, 0),
            settings_tab: 0,
            update: UpdateCheck::default(),
            integration: crate::integration::State::NotInstalled,
            installing: false,
            modal: None,
            toast: None,
            hits: Hits::default(),
            busy: HashSet::new(),
            sync_reports: HashMap::new(),
            history: HashMap::new(),
            suspend: None,
            exit: None,
            quit: false,
            tx,
            workers: Vec::new(),
            last_reload: now,
            last_scan: now,
            last_due_check: now - Duration::from_secs(60),
            opened: now,
        };
        // Who each login is, from disk. No network.
        let mut reg = Registry::load()?;
        for acc in reg.accounts.iter_mut() {
            let _ = refresh::refresh_local(acc);
            history::prune(&acc.key(), app.settings.history_days);
        }
        app.reload();
        Ok(app)
    }

    // ------------------------------------------------------------- loading

    pub fn reload(&mut self) {
        if let Ok(s) = Settings::load() {
            self.theme = Theme::detect(s.paint_background, &s.theme);
            self.settings = s;
        }
        if let Ok(reg) = Registry::load() {
            self.all = reg;
        }
        self.hs = Harness::ALL
            .into_iter()
            .map(|h| (h, HarnessSettings::load(h).unwrap_or_default()))
            .collect();
        self.scan_sessions();

        let now = Utc::now();
        let mut d = Derived::default();
        (d.logged_in, d.local) = crate::view::facts(&mut self.all);
        self.reg = Registry {
            accounts: self.all.profiles().cloned().collect(),
        };
        for h in Harness::ALL {
            let accounts: Vec<&Account> = self.reg.of(h).collect();
            d.picks
                .insert(h, suggest::rank(&accounts, &self.settings, now));
            d.bins.insert(h, manage::find_bin(h, &self.hs[&h]).ok());
        }
        d.checks = health::run(&self.all, &self.live);
        d.shells = crate::shell::all(&self.settings, &self.hs);
        // What the last check stored. The check itself runs on its own thread.
        d.newer = crate::update::enabled(&self.settings)
            .then(crate::update::known_newer)
            .flatten();
        self.derived = d;
        self.last_reload = Instant::now();
        self.last_scan = Instant::now();
        self.clamp();
    }

    /// Find the running agents and what is known about each.
    fn scan_sessions(&mut self) {
        self.live = sessions::scan(&self.all);
        let mut list: Vec<Session> = Vec::new();
        for acc in &self.all.accounts {
            for pid in self.live.get(&acc.key()).into_iter().flatten() {
                let d = sessions::details(*pid);
                list.push(Session {
                    key: acc.key(),
                    pid: *pid,
                    cwd: d.cwd,
                    started: d.started,
                });
            }
        }
        // Newest first.
        list.sort_by_key(|s| std::cmp::Reverse(s.started));
        self.sessions = list;
        self.last_scan = Instant::now();
    }

    fn load_history(&mut self) {
        self.history = self
            .reg
            .accounts
            .iter()
            .map(|a| (a.key(), history::load(&a.key())))
            .collect();
    }

    /// Compare every cswap account's links against its native dir and record
    /// what was seen.
    pub fn check_sync(&mut self) {
        let keys: Vec<String> = self.sync_keys();
        for key in keys {
            let Some(acc) = self.all.get_mut(&key) else {
                continue;
            };
            let hs = self.hs.get(&acc.harness).cloned().unwrap_or_default();
            if let Ok(report) = sync::check_account(acc, &hs) {
                self.sync_reports.insert(key, report);
            }
        }
        let now = Utc::now();
        for h in Harness::ALL {
            if self.reg.of(h).any(|a| !a.is_native()) {
                let _ = HarnessSettings::update(h, |s| s.sync.last_checked = Some(now));
            }
        }
    }

    // ---------------------------------------------------------------- rows

    /// Keys of every account, in display order.
    pub fn account_keys(&self) -> Vec<String> {
        self.reg.accounts.iter().map(|a| a.key()).collect()
    }

    /// Keys of the accounts that have links to sync: cswap's own.
    pub fn sync_keys(&self) -> Vec<String> {
        self.reg
            .accounts
            .iter()
            .filter(|a| !a.is_native())
            .map(|a| a.key())
            .collect()
    }

    /// Keys of the agents' own logins, in display order.
    pub fn root_keys(&self) -> Vec<String> {
        self.all.roots().map(|a| a.key()).collect()
    }

    pub fn rows_len(&self) -> usize {
        match self.page {
            Page::Overview | Page::Usage | Page::Accounts | Page::Analytics => {
                self.reg.accounts.len()
            }
            Page::Root => self.root_keys().len(),
            Page::Sessions => self.sessions.len(),
            Page::Sync => self.sync_keys().len(),
            Page::Resets => pages::resets::rows(self).len(),
            Page::Health => self.derived.checks.len(),
            Page::Integrations => crate::integration::ALL.len(),
            Page::Settings => pages::settings::rows(self).len(),
            // A row here is a line the text is scrolled by: one for every
            // line that does not fit the page.
            Page::Help => {
                let (w, h) = self.help_view;
                pages::help::len(self, w).saturating_sub(h) + 1
            }
        }
    }

    pub fn row(&self) -> usize {
        self.sel[self.page.index()]
    }

    fn clamp(&mut self) {
        let len = self.rows_len();
        let i = self.page.index();
        self.sel[i] = self.sel[i].min(len.saturating_sub(1));
    }

    /// The account the current page's selection points at.
    pub fn selected_key(&self) -> Option<String> {
        match self.page {
            Page::Overview | Page::Usage | Page::Accounts | Page::Analytics => {
                self.account_keys().get(self.row()).cloned()
            }
            Page::Sync => self.sync_keys().get(self.row()).cloned(),
            Page::Root => self.root_keys().get(self.row()).cloned(),
            _ => None,
        }
    }

    pub fn is_live(&self, key: &str) -> bool {
        self.live.get(key).is_some_and(|p| !p.is_empty())
    }

    /// The best account to use next for `h`, when one is usable.
    pub fn best(&self, h: Harness) -> Option<&Pick> {
        self.derived.picks.get(&h)?.iter().find(|p| p.usable)
    }

    /// Is `acc` the default of its agent: where a launch that names no
    /// account starts?
    pub fn is_default(&self, acc: &Account) -> bool {
        self.hs
            .get(&acc.harness)
            .is_some_and(|hs| crate::view::is_default(acc, hs))
    }

    pub fn say(&mut self, tone: Tone, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            tone,
            at: Instant::now(),
        });
    }

    fn fail(&mut self, e: anyhow::Error) {
        self.say(Tone::Bad, format!("{e:#}"));
    }

    // ------------------------------------------------------------ refreshes

    /// Fetch one account on its own thread. `scheduled` marks the dashboard's
    /// own round, which steps aside when another dashboard just fetched.
    fn spawn_refresh(&mut self, key: &str, scheduled: bool) {
        if !self.busy.insert(key.to_string()) {
            return;
        }
        let tx = self.tx.clone();
        let key = key.to_string();
        let settings = scheduled.then(|| self.settings.clone());
        self.workers.retain(|w| !w.is_finished());
        let worker = std::thread::spawn(move || {
            let error = refresh::fetch(&key, settings.as_ref()).err();
            let _ = tx.send(Msg::Refreshed { key, error });
        });
        self.workers.push(worker);
    }

    /// Every signed-in login, once: a root login that an account also holds
    /// is answered by that account's fetch.
    fn refresh_all(&mut self) {
        let keys: Vec<String> = self
            .all
            .accounts
            .iter()
            .filter(|a| self.derived.logged_in.contains(&a.key()))
            .filter(|a| refresh::carrier(&self.all.accounts, a).is_none())
            .map(|a| a.key())
            .collect();
        if keys.is_empty() {
            self.say(Tone::Info, "No signed-in account to refresh.");
            return;
        }
        for key in &keys {
            self.spawn_refresh(key, false);
        }
    }

    /// Called a few times a second: start the fetches that are due and keep
    /// the picture current.
    pub fn tick(&mut self) {
        let now = Instant::now();
        if self
            .toast
            .as_ref()
            .is_some_and(|t| t.at.elapsed() > Duration::from_secs(6))
        {
            self.toast = None;
        }
        if now.duration_since(self.last_reload) > Duration::from_secs(20) && self.modal.is_none() {
            self.reload();
        } else if now.duration_since(self.last_scan) > Duration::from_secs(5) {
            self.scan_sessions();
            self.clamp();
        }
        if now.duration_since(self.last_due_check) < Duration::from_secs(2) {
            return;
        }
        self.last_due_check = now;
        // Nothing is fetched in the first moments, so opening the dashboard
        // just to change a setting does not fire a round of requests.
        let warm = self.opened.elapsed() > Duration::from_millis(400);
        if !warm
            || (!self.settings.refresh_on_open && self.opened.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        let on_open = self.settings.refresh_on_open;
        let due = refresh::round(&self.all.accounts, &self.settings, Utc::now(), |a| {
            self.busy.contains(&a.key()) || (a.meta.refresh.last_attempt.is_none() && !on_open)
        });
        for key in due {
            self.spawn_refresh(&key, true);
        }
    }

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Refreshed { key, error } => {
                self.busy.remove(&key);
                self.reload();
                if self.page == Page::Analytics {
                    self.load_history();
                }
                if let Some(e) = error {
                    // Only speak up for a failure the user asked about; the
                    // page itself shows the state of every account.
                    if self.busy.is_empty() && self.toast.is_none() {
                        self.say(Tone::Warn, format!("{key}: {e}"));
                    }
                }
            }
            Msg::Integrated(done) => {
                self.installing = false;
                self.integration = crate::integration::state();
                match done {
                    Ok(d) if d.written == 0 && d.removed == 0 => self.say(
                        Tone::Good,
                        format!(
                            "The cswap layout {} is already current in herdr.",
                            d.version
                        ),
                    ),
                    Ok(d) if d.set_aside.is_empty() => self.say(
                        Tone::Good,
                        format!(
                            "Installed the cswap layout {} into herdr: {} files written.",
                            d.version, d.written
                        ),
                    ),
                    Ok(d) => self.say(
                        Tone::Info,
                        format!(
                            "Installed the cswap layout {}. Your own {} kept beside it as .mine.",
                            d.version,
                            if d.set_aside.len() == 1 {
                                "file was".to_string()
                            } else {
                                format!("{} files were", d.set_aside.len())
                            }
                        ),
                    ),
                    Err(e) => self.say(Tone::Warn, e),
                }
            }
            Msg::UpdateChecked(found) => {
                let have = crate::update::CURRENT;
                match found {
                    Ok(Some(v)) => {
                        self.say(
                            Tone::Info,
                            format!(
                                "cswap {v} is out (you have {have}). Install it with: cswap update"
                            ),
                        );
                        self.derived.newer = Some(v.clone());
                        self.update = UpdateCheck::Newer(v);
                    }
                    Ok(None) => {
                        self.say(Tone::Good, format!("cswap {have} is the newest release."));
                        self.derived.newer = None;
                        self.update = UpdateCheck::Newest;
                    }
                    Err(e) => {
                        self.say(Tone::Warn, e.clone());
                        self.update = UpdateCheck::Failed(e);
                    }
                }
            }
        }
    }

    /// Ask GitHub for the newest release on a thread of its own. The answer
    /// comes back as a message, so the dashboard never waits on the network.
    pub fn check_update(&mut self) {
        if self.update == UpdateCheck::Checking {
            return;
        }
        self.update = UpdateCheck::Checking;
        let tx = self.tx.clone();
        self.workers.retain(|w| !w.is_finished());
        self.workers.push(std::thread::spawn(move || {
            let found = crate::update::check_now().map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::UpdateChecked(found));
        }));
    }

    /// Fetch and install the herdr layout on a thread of its own: the files
    /// come from GitHub, and the dashboard never waits on the network.
    pub fn install_herdr(&mut self) {
        if self.installing {
            return;
        }
        self.installing = true;
        let tx = self.tx.clone();
        self.workers.retain(|w| !w.is_finished());
        self.workers.push(std::thread::spawn(move || {
            let done = crate::integration::install().map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Integrated(done));
        }));
    }

    fn integration_menu(&mut self) {
        use crate::integration::State;
        if self.installing {
            return;
        }
        let state = &self.integration;
        let there = *state != State::NoHerdr;
        let installed = matches!(state, State::Installed { .. });
        let items = vec![
            ChoiceItem {
                heading: None,
                label: if installed {
                    "Install again".to_string()
                } else {
                    "Install".to_string()
                },
                detail: if !there {
                    "herdr is not on this machine".to_string()
                } else if installed {
                    "fetch this version's files · your edits are kept as .mine".to_string()
                } else {
                    "fetch the layout from GitHub into herdr's folder".to_string()
                },
                enabled: there,
                key: Some('i'),
                action: Action::InstallHerdr,
            },
            ChoiceItem {
                heading: None,
                label: "Show it in herdr".to_string(),
                detail: if installed {
                    "switch the running herdr to the cswap layout".to_string()
                } else {
                    "install it first".to_string()
                },
                enabled: installed,
                key: Some('s'),
                action: Action::ShowHerdr,
            },
        ];
        self.modal = Some(Modal::Choice {
            title: "herdr".to_string(),
            hint: "herdr keeps your own layout · switch back in its settings, or with ctrl-shift-l"
                .to_string(),
            items,
            sel: 0,
        });
    }

    // ------------------------------------------------------------ navigation

    fn go(&mut self, page: Page) {
        self.page = page;
        match page {
            Page::Sync => self.check_sync(),
            Page::Analytics => self.load_history(),
            Page::Health => self.reload(),
            Page::Integrations => self.integration = crate::integration::state(),
            _ => {}
        }
        self.clamp();
    }

    fn move_row(&mut self, delta: i64) {
        let len = self.rows_len() as i64;
        if len == 0 {
            return;
        }
        let i = self.page.index();
        self.sel[i] = (self.sel[i] as i64 + delta).clamp(0, len - 1) as usize;
    }

    fn move_page(&mut self, delta: i64) {
        let n = Page::ALL.len() as i64;
        let i = (self.page.index() as i64 + delta).rem_euclid(n) as usize;
        self.go(Page::ALL[i]);
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.modal.is_some() {
            self.modal_key(key);
            return;
        }
        let on_settings = self.page == Page::Settings;
        match key.code {
            KeyCode::Esc => self.dispatch(Action::Back),
            KeyCode::Char('?') => self.dispatch(Action::Help),
            KeyCode::Char(c @ ('0'..='9' | 'i')) => {
                if let Some(page) = Page::ALL.into_iter().find(|p| p.hotkey() == c) {
                    self.go(page);
                    self.focus = Focus::Main;
                }
            }
            KeyCode::Tab => self.move_page(1),
            KeyCode::BackTab => self.move_page(-1),
            KeyCode::Char('r') => self.dispatch(Action::RefreshAll),
            KeyCode::Char('a') => self.dispatch(Action::AddAccount),
            _ if self.focus == Focus::Nav => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.move_page(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_page(1),
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Main,
                _ => {}
            },
            KeyCode::Left | KeyCode::Char('h') if on_settings => {
                self.dispatch(Action::SettingChange(-1))
            }
            KeyCode::Right if on_settings => self.dispatch(Action::SettingChange(1)),
            KeyCode::Char('[') | KeyCode::Char('T') if on_settings => {
                let n = pages::settings::TABS.len();
                self.dispatch(Action::SettingsTab((self.settings_tab + n - 1) % n));
            }
            KeyCode::Char(']') | KeyCode::Char('t') if on_settings => {
                let n = pages::settings::TABS.len();
                self.dispatch(Action::SettingsTab((self.settings_tab + 1) % n));
            }
            KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Nav,
            KeyCode::Up | KeyCode::Char('k') => self.move_row(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_row(1),
            KeyCode::PageUp => self.move_row(-8),
            KeyCode::PageDown => self.move_row(8),
            KeyCode::Home | KeyCode::Char('g') => self.move_row(-10_000),
            KeyCode::End | KeyCode::Char('G') => self.move_row(10_000),
            KeyCode::Enter | KeyCode::Char(' ') => self.dispatch(Action::Activate),
            KeyCode::Char(c) => self.page_key(c),
            _ => {}
        }
    }

    /// Keys that mean something only on some pages.
    fn page_key(&mut self, c: char) {
        let key = self.selected_key();
        let on_account = matches!(
            self.page,
            Page::Overview | Page::Usage | Page::Accounts | Page::Analytics
        );
        match (c, key) {
            ('s', _) if self.page == Page::Sync => self.dispatch(Action::SyncAll),
            ('R', Some(k)) if self.page == Page::Sync => self.dispatch(Action::Repair(k)),
            ('o', Some(k)) if on_account => self.dispatch(Action::Launch(k)),
            ('d', Some(k)) if on_account => self.dispatch(Action::SetDefault(k)),
            ('l', Some(k)) if on_account => self.dispatch(Action::Login(k)),
            ('e', Some(k)) if on_account => self.dispatch(Action::EditLabel(k)),
            ('E', Some(k)) if on_account => self.dispatch(Action::EditAliases(k)),
            ('c', Some(k)) if self.page == Page::Root => self.dispatch(Action::NewFromRoot(k)),
            ('o', Some(k)) if self.page == Page::Root => self.dispatch(Action::Launch(k)),
            ('d', Some(k)) if self.page == Page::Root => self.dispatch(Action::SetDefault(k)),
            ('u', Some(k)) if self.page == Page::Root => self.dispatch(Action::Refresh(k)),
            ('p', Some(k)) if on_account => self.dispatch(Action::EditPlan(k)),
            ('b', Some(k)) if on_account => self.dispatch(Action::EditBilling(k)),
            ('n', Some(k)) if on_account => self.dispatch(Action::AddGrant(k)),
            ('x', Some(k)) if on_account => self.dispatch(Action::Remove(k)),
            ('u', Some(k)) if on_account => self.dispatch(Action::Refresh(k)),
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                match self.hits.at(ev.column, ev.row) {
                    Some(action) => self.dispatch(action),
                    // A click outside a dialog closes it.
                    None if self.modal.is_some() => self.modal = None,
                    None => {}
                }
            }
            MouseEventKind::ScrollDown => self.scroll(1),
            MouseEventKind::ScrollUp => self.scroll(-1),
            _ => {}
        }
    }

    fn scroll(&mut self, delta: i64) {
        let list = match &mut self.modal {
            Some(Modal::Choice { items, sel, .. }) => Some((items.len(), sel)),
            Some(Modal::Ticks { items, sel, .. }) => Some((items.len(), sel)),
            _ => None,
        };
        if let Some((n, sel)) = list {
            *sel = (*sel as i64 + delta).clamp(0, (n as i64 - 1).max(0)) as usize;
        } else if self.modal.is_none() {
            self.move_row(delta);
        }
    }

    // --------------------------------------------------------------- modals

    fn modal_key(&mut self, key: KeyEvent) {
        let Some(modal) = &mut self.modal else { return };
        match modal {
            Modal::Info { .. } => self.modal = None,
            Modal::Confirm { danger, .. } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.dispatch(Action::ModalOk),
                KeyCode::Enter if !*danger => self.dispatch(Action::ModalOk),
                KeyCode::Esc | KeyCode::Char('n') => self.modal = None,
                _ => {}
            },
            Modal::Choice { items, sel, .. } => match key.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    *sel = (*sel + 1).min(items.len().saturating_sub(1))
                }
                KeyCode::Enter => {
                    let i = *sel;
                    self.dispatch(Action::ModalPick(i));
                }
                KeyCode::Char(c) => {
                    if let Some(i) = items.iter().position(|it| it.key == Some(c)) {
                        self.dispatch(Action::ModalPick(i));
                    }
                }
                _ => {}
            },
            Modal::Ticks { items, sel, .. } => match key.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    *sel = (*sel + 1).min(items.len().saturating_sub(1))
                }
                KeyCode::Char(' ') | KeyCode::Char('x') => {
                    let i = *sel;
                    self.dispatch(Action::Tick(i));
                }
                KeyCode::Enter | KeyCode::Char('s') => self.dispatch(Action::ModalOk),
                _ => {}
            },
            Modal::Input { value, error, .. } => match key.code {
                KeyCode::Esc => self.modal = None,
                KeyCode::Enter => self.dispatch(Action::ModalOk),
                KeyCode::Backspace => {
                    value.pop();
                    *error = None;
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    value.clear();
                    *error = None;
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if value.chars().count() < 200 {
                        value.push(c);
                    }
                    *error = None;
                }
                _ => {}
            },
        }
    }

    fn input(&mut self, title: &str, prompt: &str, value: &str, hint: &str, then: Then) {
        self.modal = Some(Modal::Input {
            title: title.to_string(),
            prompt: prompt.to_string(),
            value: value.to_string(),
            hint: hint.to_string(),
            error: None,
            then,
        });
    }

    fn confirm(&mut self, title: &str, body: Vec<String>, yes: &str, danger: bool, action: Action) {
        self.modal = Some(Modal::Confirm {
            title: title.to_string(),
            body,
            yes: yes.to_string(),
            danger,
            action: Box::new(action),
        });
    }

    fn input_error(&mut self, text: String) {
        if let Some(Modal::Input { error, .. }) = &mut self.modal {
            *error = Some(text);
        }
    }

    /// The text of an input modal was confirmed.
    fn submit(&mut self, then: Then, value: String) {
        let value = value.trim().to_string();
        match then {
            Then::CreateAccount(h) => match manage::create(h, &value) {
                Ok(acc) => {
                    self.modal = None;
                    self.reload();
                    self.suspend = Some(Suspend::Login {
                        key: acc.key(),
                        created: true,
                    });
                }
                Err(e) => self.input_error(format!("{e:#}")),
            },
            Then::Label(key) => {
                let result = match self.all.get_mut(&key) {
                    Some(acc) => manage::rename(acc, &value),
                    None => Ok(()),
                };
                match result {
                    Ok(()) => {
                        self.modal = None;
                        self.reload();
                        self.say(Tone::Good, format!("Renamed to '{value}'."));
                    }
                    Err(e) => self.input_error(format!("{e:#}")),
                }
            }
            Then::Aliases(key) => {
                let list: Vec<String> = value
                    .split([',', ' '])
                    .map(|a| a.trim().to_string())
                    .filter(|a| !a.is_empty())
                    .collect();
                let result = match self.all.get_mut(&key) {
                    Some(acc) => manage::set_aliases(acc, &list).map(|_| acc.title()),
                    None => Ok(String::new()),
                };
                match result {
                    Ok(name) => {
                        self.modal = None;
                        self.reload();
                        let text = if list.is_empty() {
                            format!("{name} has no alias now.")
                        } else {
                            format!("{name} also answers to {}.", list.join(", "))
                        };
                        self.say(Tone::Good, text);
                    }
                    Err(e) => self.input_error(format!("{e:#}")),
                }
            }
            Then::Plan(key) => {
                self.modal = None;
                self.update_account(&key, |m| {
                    if value.is_empty() {
                        // Back to whatever the vendor reports.
                        m.plan_source = Source::Api;
                    } else {
                        m.plan = Some(value.clone());
                        m.plan_source = Source::Manual;
                    }
                });
            }
            Then::BillingDate(key) => match parse_date(&value) {
                Err(e) => self.input_error(e),
                Ok(None) => {
                    self.modal = None;
                    self.update_account(&key, |m| m.billing = Default::default());
                    self.say(Tone::Good, format!("{key}: renewal cleared"));
                }
                Ok(Some(date)) => {
                    let item = |label: &str, detail: &str, k: char, cycle| ChoiceItem {
                        heading: None,
                        label: label.to_string(),
                        detail: detail.to_string(),
                        enabled: true,
                        key: Some(k),
                        action: Action::BillingCycle(key.clone(), date, cycle),
                    };
                    self.modal = Some(Modal::Choice {
                        title: "How often does it renew?".to_string(),
                        hint: format!("renewal date {}", date.format("%-d %b %Y")),
                        items: vec![
                            item(
                                "Monthly",
                                "the date repeats every month",
                                'm',
                                Some(Cycle::Monthly),
                            ),
                            item(
                                "Yearly",
                                "the date repeats every year",
                                'y',
                                Some(Cycle::Yearly),
                            ),
                            item("Just this date", "no repeat", 'o', None),
                        ],
                        sel: 0,
                    });
                }
            },
            Then::GrantExpiry(key) => match parse_date(&value) {
                Err(e) => self.input_error(e),
                Ok(date) => {
                    self.modal = None;
                    let grant = Grant {
                        scope: "limits".to_string(),
                        count: 1,
                        expires_at: date.map(end_of_day),
                        note: "noted by hand".to_string(),
                        source: Source::Manual,
                        usable_now: None,
                    };
                    self.update_account(&key, |m| m.grants.push(grant));
                    self.say(Tone::Good, format!("{key}: reset noted"));
                }
            },
            Then::Setting(id) => match pages::settings::set_text(self, id, &value) {
                Ok(()) => {
                    self.modal = None;
                    self.reload();
                }
                Err(e) => self.input_error(e),
            },
        }
    }

    fn update_account(&mut self, key: &str, f: impl FnOnce(&mut store::AccountMeta)) {
        let result = match self.all.get_mut(key) {
            Some(acc) => {
                // The native login has no cswap dir until something is stored.
                let _ = crate::fsx::private_dir(&acc.dir());
                acc.update(f)
            }
            None => Ok(()),
        };
        match result {
            Ok(()) => self.reload(),
            Err(e) => self.fail(e),
        }
    }

    // -------------------------------------------------------------- actions

    pub fn dispatch(&mut self, action: Action) {
        match action {
            // Escape steps back one level: out of the page to the menu, and
            // from the menu out of cswap.
            Action::Back => match self.focus {
                Focus::Main => self.focus = Focus::Nav,
                Focus::Nav => self.quit = true,
            },
            Action::Go(page) => {
                self.go(page);
                self.focus = Focus::Main;
            }
            Action::Row(i) => {
                self.focus = Focus::Main;
                if self.row() == i {
                    self.dispatch(Action::Activate);
                } else {
                    let p = self.page.index();
                    self.sel[p] = i;
                    self.clamp();
                }
            }
            Action::Activate => match self.page {
                Page::Overview | Page::Usage | Page::Accounts | Page::Analytics => {
                    if let Some(k) = self.selected_key() {
                        self.dispatch(Action::AccountMenu(k));
                    }
                }
                Page::Sync => {
                    if let Some(k) = self.selected_key() {
                        self.dispatch(Action::SyncOne(k));
                    }
                }
                Page::Settings => self.dispatch(Action::SettingChange(0)),
                Page::Health => {
                    if let Some(c) = self.derived.checks.get(self.row()) {
                        let mut body = vec![c.title.clone()];
                        if !c.detail.is_empty() {
                            body.push(String::new());
                            body.push(c.detail.clone());
                        }
                        self.modal = Some(Modal::Info {
                            title: c.area.clone(),
                            body,
                        });
                    }
                }
                Page::Root => {
                    if let Some(k) = self.selected_key() {
                        self.dispatch(Action::AccountMenu(k));
                    }
                }
                Page::Integrations => self.dispatch(Action::IntegrationMenu),
                Page::Resets | Page::Sessions | Page::Help => {}
            },
            Action::IntegrationMenu => self.integration_menu(),
            Action::InstallHerdr => {
                self.modal = None;
                self.install_herdr();
            }
            Action::ShowHerdr => {
                self.modal = None;
                match crate::integration::show() {
                    Ok(()) => self.say(Tone::Good, "herdr shows the cswap layout now.".to_string()),
                    Err(e) => self.say(Tone::Warn, format!("{e:#}")),
                }
            }
            // To the Help page, and from it back to where `?` was pressed.
            Action::Help => {
                if self.page == Page::Help {
                    self.go(self.before_help);
                } else {
                    self.before_help = self.page;
                    self.go(Page::Help);
                    self.focus = Focus::Main;
                }
            }
            Action::RefreshAll => self.refresh_all(),
            Action::Refresh(key) => {
                if self.derived.logged_in.contains(&key) {
                    self.modal = None;
                    self.spawn_refresh(&key, false);
                } else {
                    self.say(Tone::Warn, format!("{key} is not signed in."));
                }
            }

            // ---- accounts
            Action::AddAccount => self.add_account(),
            Action::NewAccountName(h, suggestion) => self.input(
                &format!("New {} account", h.name()),
                "Name",
                &suggestion,
                "a short name such as work, used as --cswap=work · Enter goes on to the sign-in",
                Then::CreateAccount(h),
            ),
            Action::NewFromRoot(key) => self.new_from_root(&key),
            Action::AccountMenu(key) => self.account_menu(&key),
            Action::Login(key) => self.login(&key),
            Action::DoLogin(key) => {
                self.modal = None;
                self.suspend = Some(Suspend::Login {
                    key,
                    created: false,
                });
            }
            Action::Remove(key) => self.remove(&key),
            Action::DoRemove(key) => {
                self.modal = None;
                let result = match self.all.get(&key) {
                    Some(acc) => manage::remove(acc, &self.live),
                    None => Ok(()),
                };
                match result {
                    Ok(()) => {
                        self.reload();
                        self.say(Tone::Good, format!("{key} removed."));
                    }
                    Err(e) => self.fail(e),
                }
            }
            Action::EditLabel(key) => {
                let current = self.all.get(&key).map(|a| a.meta.label.clone()).unwrap_or_default();
                self.input(
                    &format!("Rename {current}"),
                    "Name",
                    &current,
                    "letters, digits, - _ . · used as --cswap=<name>",
                    Then::Label(key),
                );
            }
            Action::EditAliases(key) => {
                let (name, current) = self
                    .all
                    .get(&key)
                    .map(|a| (a.title(), a.meta.aliases.join(", ")))
                    .unwrap_or_default();
                self.input(
                    &format!("Aliases for {name}"),
                    "Aliases",
                    &current,
                    "extra names it answers to, separated by commas · empty clears them",
                    Then::Aliases(key),
                );
            }
            Action::EditPlan(key) => {
                let current = self
                    .reg
                    .get(&key)
                    .filter(|a| a.meta.plan_source == Source::Manual)
                    .and_then(|a| a.meta.plan.clone())
                    .unwrap_or_default();
                self.input(
                    &format!("Plan for {key}"),
                    "Plan name",
                    &current,
                    "such as Ultra · empty goes back to what the vendor reports",
                    Then::Plan(key),
                );
            }
            Action::EditBilling(key) => {
                let current = self
                    .reg
                    .get(&key)
                    .and_then(|a| a.meta.billing.renews_on)
                    .map(|d| d.to_string())
                    .unwrap_or_default();
                self.input(
                    &format!("Renewal for {key}"),
                    "Next renewal date",
                    &current,
                    "YYYY-MM-DD, or 30d for thirty days from today · empty clears it",
                    Then::BillingDate(key),
                );
            }
            Action::BillingCycle(key, date, cycle) => {
                let item = |label: &str, detail: &str, k: char, cancelled| ChoiceItem {
                    heading: None,
                    label: label.to_string(),
                    detail: detail.to_string(),
                    enabled: true,
                    key: Some(k),
                    action: Action::BillingSave(key.clone(), date, cycle, cancelled),
                };
                self.modal = Some(Modal::Choice {
                    title: "What happens on that date?".to_string(),
                    hint: String::new(),
                    items: vec![
                        item("It renews", "the plan carries on", 'r', false),
                        item(
                            "It ends",
                            "cancelled: cswap will suggest using it up first",
                            'e',
                            true,
                        ),
                    ],
                    sel: 0,
                });
            }
            Action::BillingSave(key, date, cycle, cancelled) => {
                self.modal = None;
                self.update_account(&key, |m| {
                    m.billing = store::Billing {
                        cycle,
                        renews_on: Some(date),
                        cancelled,
                        source: Source::Manual,
                    }
                });
                self.say(Tone::Good, format!("{key}: renewal saved"));
            }
            Action::AddGrant(key) => self.input(
                &format!("Note a reset for {key}"),
                "It expires on",
                "",
                "YYYY-MM-DD, or 30d · empty if it does not expire · cswap only tracks it, it cannot spend it",
                Then::GrantExpiry(key),
            ),
            Action::ClearGrants(key) => {
                self.modal = None;
                self.update_account(&key, |m| m.grants.clear());
                self.say(Tone::Good, format!("{key}: hand-noted resets cleared"));
            }
            Action::SetDefault(key) => {
                self.modal = None;
                let Some(acc) = self.all.get(&key) else { return };
                let (h, id) = (acc.harness, acc.meta.id.to_string());
                let bin = h.spec().bins[0];
                let who = if acc.is_native() {
                    format!("{}'s root login", h.name())
                } else {
                    acc.title()
                };
                if self.is_default(acc) {
                    self.say(
                        Tone::Info,
                        format!("{who} is already the default for `cswap {bin}`."),
                    );
                    return;
                }
                match manage::set_start(h, &id) {
                    Ok(_) => {
                        self.reload();
                        self.say(
                            Tone::Good,
                            format!("{who} is now the default for `cswap {bin}`."),
                        );
                    }
                    Err(e) => self.fail(e),
                }
            }
            Action::Launch(key) => {
                self.modal = None;
                if !self.derived.logged_in.contains(&key) {
                    self.say(Tone::Warn, format!("{key} is not signed in. Press l to sign in."));
                    return;
                }
                self.exit = Some(Exit::Launch { key });
                self.quit = true;
            }

            // ---- sync
            Action::SyncAll => self.sync_all(),
            Action::SyncOne(key) => {
                self.sync_one(&key);
                self.check_sync();
            }
            Action::Repair(key) => {
                let shadowed = self
                    .sync_reports
                    .get(&key)
                    .map(|r| r.names(sync::State::Shadowed))
                    .unwrap_or_default();
                if shadowed.is_empty() {
                    self.say(Tone::Info, format!("{key} has nothing to repair."));
                } else if self.is_live(&key) {
                    self.say(Tone::Warn, format!("{key} has a session running. Close it first."));
                } else {
                    self.confirm(
                        &format!("Repair {key}"),
                        vec![
                            format!("{} keeps its own copy of:", key),
                            format!("  {}", shadowed.join(", ")),
                            String::new(),
                            "They will be moved to ~/.cswap/parked/ and replaced by links to the"
                                .to_string(),
                            "agent's own files. Nothing is deleted and nothing is merged."
                                .to_string(),
                        ],
                        "Move aside and relink",
                        false,
                        Action::DoRepair(key),
                    );
                }
            }
            Action::DoRepair(key) => {
                self.modal = None;
                let Some(acc) = self.all.get(&key) else { return };
                let hs = self.hs.get(&acc.harness).cloned().unwrap_or_default();
                match sync::repair(acc.harness, &key, &acc.config_dir(), &hs.sync.exclude) {
                    Ok(Some(park)) => {
                        self.check_sync();
                        self.reload();
                        self.say(
                            Tone::Good,
                            format!("Moved to {}", crate::paths::tilde(&park)),
                        );
                    }
                    Ok(None) => self.say(Tone::Info, "Nothing needed repair."),
                    Err(e) => self.fail(e),
                }
            }

            // ---- settings
            Action::SettingsTab(i) => {
                self.settings_tab = i.min(pages::settings::TABS.len() - 1);
                self.sel[Page::Settings.index()] = 0;
                self.focus = Focus::Main;
            }
            Action::SettingChange(dir) => pages::settings::change(self, dir),

            // ---- modal plumbing
            Action::ModalCancel => self.modal = None,
            Action::ModalPick(i) => {
                if let Some(Modal::Choice { items, sel, .. }) = &mut self.modal {
                    *sel = i;
                    if let Some(item) = items.get(i).filter(|it| it.enabled).cloned() {
                        self.modal = None;
                        self.dispatch(item.action);
                    }
                }
            }
            Action::Ignore => {}
            Action::Tick(i) => {
                if let Some(Modal::Ticks { items, sel, .. }) = &mut self.modal {
                    if let Some(item) = items.get_mut(i) {
                        *sel = i;
                        if item.enabled {
                            item.on = !item.on;
                        }
                    }
                }
            }
            Action::ModalOk => match self.modal.clone() {
                Some(Modal::Confirm { action, .. }) => {
                    self.modal = None;
                    self.dispatch(*action);
                }
                Some(Modal::Input { then, value, .. }) => self.submit(then, value),
                Some(Modal::Choice { sel, .. }) => self.dispatch(Action::ModalPick(sel)),
                Some(Modal::Ticks { items, then, .. }) => {
                    self.modal = None;
                    self.save_ticks(then, items);
                }
                _ => self.modal = None,
            },
        }
    }

    fn add_account(&mut self) {
        let items = Harness::ALL
            .into_iter()
            .enumerate()
            .map(|(i, h)| {
                let installed = self.derived.bins.get(&h).is_some_and(|b| b.is_some());
                let count = self.reg.of(h).filter(|a| !a.is_native()).count();
                let mut detail = if !installed {
                    format!("`{}` is not installed", h.spec().bins[0])
                } else if count == 0 {
                    "no accounts yet".to_string()
                } else {
                    format!("{count} account{}", if count == 1 { "" } else { "s" })
                };
                if h.spec().experimental && installed {
                    detail.push_str(" · experimental");
                }
                ChoiceItem {
                    heading: None,
                    label: h.name().to_string(),
                    detail,
                    enabled: installed && h.wrappable(),
                    key: char::from_digit(i as u32 + 1, 10),
                    action: Action::NewAccountName(h, String::new()),
                }
            })
            .collect();
        self.modal = Some(Modal::Choice {
            title: "Add an account".to_string(),
            hint: "pick the agent · then name the account · then the agent's own sign-in runs"
                .to_string(),
            items,
            sel: 0,
        });
    }

    /// Start an account for the login an agent's root holds. The login is
    /// not copied: one login in two places stops working as soon as either
    /// side renews it. The agent's sign-in runs, and you sign in again.
    fn new_from_root(&mut self, key: &str) {
        let Some(acc) = self.all.get(key) else { return };
        let h = acc.harness;
        if !self.derived.logged_in.contains(key) {
            self.say(
                Tone::Warn,
                format!(
                    "{} is not signed in, so there is no login to make an account from.",
                    h.name()
                ),
            );
            return;
        }
        if let Some(twin) = manage::duplicate_of(&self.all, acc) {
            self.say(
                Tone::Info,
                format!("{} already holds this login.", twin.title()),
            );
            return;
        }
        if !h.wrappable() || self.derived.bins.get(&h).is_none_or(|b| b.is_none()) {
            self.say(
                Tone::Warn,
                format!("cswap cannot hold accounts for {}.", h.name()),
            );
            return;
        }
        let suggestion = manage::suggest_name(&self.all, h, acc.email());
        let who = match acc.email() {
            "" => "the same login".to_string(),
            e => e.to_string(),
        };
        self.input(
            &format!("Account from {}'s root login", h.name()),
            "Name",
            &suggestion,
            &format!("next you sign in as {who} again · cswap never copies a login"),
            Then::CreateAccount(h),
        );
    }

    /// Tick the agents a shell wraps. A tick is what the shell does now: an
    /// agent counts only once the shell's rc file loads cswap.
    pub fn shell_menu(&mut self, at: usize) {
        let Some(shell) = self.derived.shells.get(at) else {
            return;
        };
        let items = Harness::ALL
            .into_iter()
            .map(|h| {
                let installed = self.derived.bins.get(&h).is_some_and(|b| b.is_some());
                let on = shell.wraps(h);
                TickItem {
                    id: h.id().to_string(),
                    label: h.spec().bins[0].to_string(),
                    detail: if installed {
                        h.name().to_string()
                    } else {
                        format!("{} · not installed", h.name())
                    },
                    enabled: installed,
                    was: on,
                    on,
                }
            })
            .collect();
        let hint = match (&shell.rc, shell.loaded) {
            (Some(rc), Some(true)) => format!("{} loads cswap", paths::tilde(rc)),
            (Some(rc), _) => format!("saving adds one line to {}", paths::tilde(rc)),
            (None, _) => {
                "cswap cannot write this shell's profile: it shows the line to add".to_string()
            }
        };
        self.modal = Some(Modal::Ticks {
            title: format!("Agents wrapped in {}", shell.name),
            hint,
            items,
            sel: 0,
            then: Ticks::Shell(shell.name.to_string()),
        });
    }

    fn save_ticks(&mut self, then: Ticks, items: Vec<TickItem>) {
        if items.iter().all(|it| it.on == it.was) {
            self.say(Tone::Info, "Nothing changed.");
            return;
        }
        match then {
            Ticks::Shell(name) => {
                let agents: Vec<Harness> = Harness::ALL
                    .into_iter()
                    .filter(|h| items.iter().any(|it| it.on && it.id == h.id()))
                    .collect();
                let saved = crate::shell::set_chosen(&name, &agents);
                self.reload();
                let saved = match saved {
                    Ok(saved) => saved,
                    Err(e) => return self.fail(e),
                };
                let bins: Vec<&str> = agents.iter().map(|h| h.spec().bins[0]).collect();
                let what = if bins.is_empty() {
                    format!("{name} wraps no agent now.")
                } else {
                    format!("{name} now wraps {}.", bins.join(", "))
                };
                if let Some(line) = saved.by_hand {
                    self.modal = Some(Modal::Info {
                        title: what,
                        body: vec![
                            format!("Add this line to {name}'s profile, then open a new terminal:"),
                            String::new(),
                            line,
                        ],
                    });
                    return;
                }
                let added = match saved.added_to {
                    Some(rc) => format!(" Added the cswap line to {}.", paths::tilde(&rc)),
                    None => String::new(),
                };
                self.say(
                    Tone::Good,
                    format!("{what}{added} Open a new terminal to use it."),
                );
            }
        }
    }

    /// What can be done with an agent's own login.
    fn root_menu(&mut self, key: &str) {
        let Some(acc) = self.all.get(key) else { return };
        let h = acc.harness;
        let bin = h.spec().bins[0];
        let signed_in = self.derived.logged_in.contains(key);
        let is_default = self.is_default(acc);
        let twin = manage::duplicate_of(&self.all, acc).map(|t| t.title());
        let k = key.to_string();
        let item =
            |label: String, detail: String, key: char, enabled: bool, action: Action| ChoiceItem {
                heading: None,
                label,
                detail,
                enabled,
                key: Some(key),
                action,
            };
        let items = grouped(vec![
            (
                "LAUNCH",
                vec![
                    item(
                        format!("Open {bin} on its root login"),
                        "closes the dashboard and starts the agent".to_string(),
                        'o',
                        signed_in,
                        Action::Launch(k.clone()),
                    ),
                    item(
                        "Set as default".to_string(),
                        default_detail(bin, is_default),
                        'd',
                        !is_default,
                        Action::SetDefault(k.clone()),
                    ),
                ],
            ),
            (
                "USAGE",
                vec![item(
                    "Refresh usage".to_string(),
                    String::new(),
                    'u',
                    signed_in,
                    Action::Refresh(k.clone()),
                )],
            ),
            (
                "ACCOUNT",
                vec![item(
                    "Create an account from this login".to_string(),
                    match &twin {
                        Some(t) => format!("{t} already holds it"),
                        None if !signed_in => "not signed in".to_string(),
                        None => "name it, then sign in again".to_string(),
                    },
                    'c',
                    signed_in && twin.is_none() && h.wrappable(),
                    Action::NewFromRoot(k),
                )],
            ),
        ]);
        let mut hint = acc.email().to_string();
        if hint.is_empty() {
            hint = "not signed in".to_string();
        }
        self.modal = Some(Modal::Choice {
            title: format!("{} · root login", h.name()),
            hint,
            items,
            sel: 0,
        });
    }

    fn account_menu(&mut self, key: &str) {
        let Some(acc) = self.all.get(key) else { return };
        if acc.is_native() {
            self.root_menu(key);
            return;
        }
        let h = acc.harness;
        let signed_in = self.derived.logged_in.contains(key);
        let is_default = self.is_default(acc);
        let bin = h.spec().bins[0];
        let k = key.to_string();
        let item =
            |label: &str, detail: &str, key: char, enabled: bool, action: Action| ChoiceItem {
                heading: None,
                label: label.to_string(),
                detail: detail.to_string(),
                enabled,
                key: Some(key),
                action,
            };
        let mut usage = vec![
            item(
                "Refresh usage",
                "",
                'u',
                signed_in,
                Action::Refresh(k.clone()),
            ),
            item(
                "Note an available reset",
                "for resets the vendor does not report",
                'n',
                true,
                Action::AddGrant(k.clone()),
            ),
        ];
        if !acc.meta.grants.is_empty() {
            usage.push(item(
                "Clear hand-noted resets",
                "",
                'c',
                true,
                Action::ClearGrants(k.clone()),
            ));
        }
        let items = grouped(vec![
            (
                "LAUNCH",
                vec![
                    item(
                        &format!("Open {bin} on this account"),
                        "closes the dashboard and starts the agent",
                        'o',
                        signed_in,
                        Action::Launch(k.clone()),
                    ),
                    item(
                        "Set as default",
                        &default_detail(bin, is_default),
                        'd',
                        !is_default,
                        Action::SetDefault(k.clone()),
                    ),
                ],
            ),
            ("USAGE", usage),
            (
                "ACCOUNT",
                vec![
                    item(
                        if signed_in {
                            "Sign in again"
                        } else {
                            "Sign in"
                        },
                        "",
                        'l',
                        true,
                        Action::Login(k.clone()),
                    ),
                    item("Rename", "", 'e', true, Action::EditLabel(k.clone())),
                    item(
                        "Set aliases",
                        "extra names it answers to",
                        'E',
                        true,
                        Action::EditAliases(k.clone()),
                    ),
                    item("Set plan name", "", 'p', true, Action::EditPlan(k.clone())),
                    item(
                        "Set renewal date",
                        "monthly or yearly",
                        'b',
                        true,
                        Action::EditBilling(k.clone()),
                    ),
                ],
            ),
            // Set apart, so it is not picked by a slip.
            (
                "",
                vec![item(
                    "Remove account",
                    "deletes its login",
                    'x',
                    true,
                    Action::Remove(k),
                )],
            ),
        ]);
        let mut hint = acc.email().to_string();
        if hint.is_empty() {
            hint = key.to_string();
        }
        self.modal = Some(Modal::Choice {
            title: format!("{} · {}", h.name(), acc.title()),
            hint,
            items,
            sel: 0,
        });
    }

    fn login(&mut self, key: &str) {
        let Some(acc) = self.all.get(key) else { return };
        if acc.is_native() {
            self.say(
                Tone::Info,
                format!(
                    "{key} is {}'s own login. Run `{}` and sign in there.",
                    acc.harness.name(),
                    acc.harness.spec().bins[0]
                ),
            );
            return;
        }
        if self.is_live(key) {
            self.say(
                Tone::Warn,
                format!("{key} has a session running. Close it first."),
            );
            return;
        }
        if self.derived.logged_in.contains(key) {
            self.confirm(
                &format!("Sign in to {key} again"),
                vec![
                    format!("{} is signed in as {}.", acc.title(), display_email(acc)),
                    String::new(),
                    format!(
                        "The dashboard steps aside for {}'s own sign-in.",
                        acc.harness.name()
                    ),
                    "Signing in as someone else replaces this login.".to_string(),
                ],
                "Sign in",
                false,
                Action::DoLogin(key.to_string()),
            );
        } else {
            self.dispatch(Action::DoLogin(key.to_string()));
        }
    }

    fn remove(&mut self, key: &str) {
        let Some(acc) = self.all.get(key) else { return };
        if acc.is_native() {
            self.say(
                Tone::Info,
                format!(
                    "{key} is {}'s own login; cswap cannot remove it.",
                    acc.harness.name()
                ),
            );
            return;
        }
        if self.is_live(key) {
            self.say(
                Tone::Warn,
                format!("{key} has a session running. Close it first."),
            );
            return;
        }
        self.confirm(
            &format!("Remove {key}"),
            vec![
                format!("{} ({})", acc.title(), display_email(acc)),
                String::new(),
                "Its login is deleted; you would have to sign in again.".to_string(),
                format!(
                    "Your {} data is not touched: the account only links to it.",
                    crate::paths::tilde(&acc.harness.native_dir())
                ),
            ],
            "Remove",
            true,
            Action::DoRemove(key.to_string()),
        );
    }

    fn sync_one(&mut self, key: &str) -> Option<Report> {
        let acc = self.all.get_mut(key)?;
        let hs = self.hs.get(&acc.harness).cloned().unwrap_or_default();
        let h = acc.harness;
        match sync::sync_account(acc, &hs) {
            Ok(report) => {
                let _ = HarnessSettings::update(h, |s| s.sync.last_synced = Some(Utc::now()));
                let left = report.count(sync::State::Shadowed);
                let text = if left == 0 {
                    format!("{key}: {} links in sync", report.count(sync::State::Linked))
                } else {
                    format!("{key}: synced, {left} file(s) need a repair (R)")
                };
                self.say(if left == 0 { Tone::Good } else { Tone::Warn }, text);
                Some(report)
            }
            Err(e) => {
                self.fail(e);
                None
            }
        }
    }

    fn sync_all(&mut self) {
        let keys = self.sync_keys();
        if keys.is_empty() {
            self.say(
                Tone::Info,
                "No cswap accounts to sync yet. Press a to add one.",
            );
            return;
        }
        let (mut links, mut shadowed) = (0, 0);
        for key in &keys {
            if let Some(r) = self.sync_one(key) {
                links += r.count(sync::State::Linked);
                shadowed += r.count(sync::State::Shadowed);
            }
        }
        self.check_sync();
        self.reload();
        let text = format!(
            "Synced {} account{}: {links} links{}",
            keys.len(),
            if keys.len() == 1 { "" } else { "s" },
            if shadowed > 0 {
                format!(", {shadowed} file(s) need a repair")
            } else {
                String::new()
            }
        );
        self.say(if shadowed > 0 { Tone::Warn } else { Tone::Good }, text);
    }

    /// The agent's sign-in has finished and the dashboard is back.
    pub fn after_login(&mut self, key: &str, created: bool, result: Result<bool>) {
        self.reload();
        match result {
            Ok(true) => {
                let Some(acc) = self.all.get(key) else { return };
                let name = acc.title();
                let who = match acc.email() {
                    "" => String::new(),
                    e => format!(" as {e}"),
                };
                let twin = manage::duplicate_of(&self.all, acc).map(|d| d.title());
                match twin {
                    // One account per login. The agent's own default is the
                    // one place the same login may also sit.
                    Some(other) if created => {
                        let _ = manage::remove(acc, &Live::new());
                        self.reload();
                        self.say(
                            Tone::Bad,
                            format!("{other} already holds that login, so '{name}' was not added."),
                        );
                        return;
                    }
                    Some(other) => self.say(
                        Tone::Bad,
                        format!("{name} is signed in{who}, the login {other} already holds. Remove one of the two."),
                    ),
                    None => self.say(Tone::Good, format!("{name} is signed in{who}.")),
                }
                self.page = Page::Accounts;
                self.focus = Focus::Main;
                if let Some(i) = self.account_keys().iter().position(|k| k == key) {
                    self.sel[Page::Accounts.index()] = i;
                }
                self.spawn_refresh(key, false);
            }
            Ok(false) if created => {
                if let Some(acc) = self.all.get(key) {
                    let _ = manage::remove(acc, &Live::new());
                }
                self.reload();
                self.say(Tone::Warn, "Sign-in was not completed. Nothing was added.");
            }
            Ok(false) => self.say(
                Tone::Warn,
                format!("Sign-in was not completed. {key} is unchanged."),
            ),
            Err(e) => {
                if created {
                    if let Some(acc) = self.all.get(key) {
                        let _ = manage::remove(acc, &Live::new());
                    }
                    self.reload();
                }
                self.fail(e);
            }
        }
    }
}

/// A test's fabricated home goes away with the test, so its fetches have to
/// finish first. The real dashboard never waits: quitting must be instant.
#[cfg(test)]
impl Drop for App {
    fn drop(&mut self) {
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// An account's email, or a plain word when it is not known.
pub fn display_email(acc: &Account) -> String {
    match acc.email() {
        "" => "an unknown login".to_string(),
        e => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_parse_as_iso_or_days_ahead() {
        assert_eq!(parse_date("").unwrap(), None);
        assert_eq!(
            parse_date("2026-12-01").unwrap(),
            NaiveDate::from_ymd_opt(2026, 12, 1)
        );
        assert_eq!(
            parse_date("30d").unwrap(),
            Some(store::today() + ChronoDuration::days(30))
        );
        assert!(parse_date("next week").is_err());
    }
}
