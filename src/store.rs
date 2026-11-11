//! Everything cswap keeps on disk, as typed JSON: the global settings, one
//! settings file per harness and one per account.
//!
//! Every write goes through a locked read-modify-write (`update_*`), because
//! the dashboard and a wrapped launch may touch the same file at once.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Datelike, Months, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::harness::Harness;
use crate::model::{Grant, Snapshot, Source};
use crate::{fsx, paths};

pub const SCHEMA: u32 = 1;

// ------------------------------------------------------------ global settings

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema: u32,
    /// How often the open dashboard refetches usage. Wrapped launches never
    /// fetch; they read what the dashboard last stored.
    pub refresh_minutes: u64,
    /// Fetch stale accounts as soon as the dashboard opens.
    pub refresh_on_open: bool,
    pub mouse: bool,
    pub clock_24h: bool,
    /// Paint cswap's own dark background. Off keeps the terminal's.
    pub paint_background: bool,
    /// Days of usage samples kept for the analytics page.
    pub history_days: u32,
    /// Auto-pick skips an account whose weekly usage is at or above this.
    pub auto_skip_above_pct: f64,
    /// Check GitHub for a newer cswap, at most once a day.
    pub update_check: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            schema: SCHEMA,
            refresh_minutes: 10,
            refresh_on_open: true,
            mouse: true,
            clock_24h: true,
            paint_background: true,
            history_days: 90,
            auto_skip_above_pct: 97.0,
            update_check: true,
        }
    }
}

/// The lowest refresh interval offered. The usage endpoints are rate limited
/// per token, so the dashboard does not poll faster than this.
pub const MIN_REFRESH_MINUTES: u64 = 2;

impl Settings {
    pub fn load() -> Result<Settings> {
        let mut s: Settings = fsx::read_json(&paths::settings_file())?.unwrap_or_default();
        s.refresh_minutes = s.refresh_minutes.max(MIN_REFRESH_MINUTES);
        Ok(s)
    }

    pub fn update(f: impl FnOnce(&mut Settings)) -> Result<Settings> {
        fsx::with_lock(&paths::root(), || {
            let mut s = Settings::load()?;
            f(&mut s);
            s.refresh_minutes = s.refresh_minutes.max(MIN_REFRESH_MINUTES);
            fsx::write_json(&paths::settings_file(), &s)?;
            Ok(s)
        })
    }
}

// ----------------------------------------------------------- harness settings

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncSettings {
    /// Mirror the native config dir into each account before every launch.
    pub enabled: bool,
    /// Extra names kept private to each account, on top of the built-in list.
    pub exclude: Vec<String>,
    pub last_synced: Option<DateTime<Utc>>,
    pub last_checked: Option<DateTime<Utc>>,
}

impl Default for SyncSettings {
    fn default() -> Self {
        SyncSettings {
            enabled: true,
            exclude: Vec::new(),
            last_synced: None,
            last_checked: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HarnessSettings {
    pub schema: u32,
    /// When off, `cswap <harness>` hands straight to the real binary unless an
    /// account is named with `--cswap=`.
    pub wrapped: bool,
    /// Which account a bare launch uses: `native`, `auto`, or an account id.
    pub default: String,
    /// Path to the real binary, when it is not the first one on `PATH`.
    pub binary: Option<String>,
    /// Print one line on stderr saying which account a launch uses.
    pub announce: bool,
    /// Identify as the agent's own CLI on the one read that needs it. Claude
    /// lists reset grants only to its CLI; off means no grant details.
    pub cli_identity: bool,
    pub sync: SyncSettings,
}

impl Default for HarnessSettings {
    fn default() -> Self {
        HarnessSettings {
            schema: SCHEMA,
            wrapped: true,
            default: "native".to_string(),
            binary: None,
            announce: true,
            cli_identity: true,
            sync: SyncSettings::default(),
        }
    }
}

impl HarnessSettings {
    pub fn load(h: Harness) -> Result<HarnessSettings> {
        Ok(fsx::read_json(&paths::harness_settings(h.id()))?.unwrap_or_default())
    }

    pub fn update(h: Harness, f: impl FnOnce(&mut HarnessSettings)) -> Result<HarnessSettings> {
        let file = paths::harness_settings(h.id());
        let dir = file.parent().expect("has a parent").to_path_buf();
        fsx::with_lock(&dir, || {
            let mut s = HarnessSettings::load(h)?;
            f(&mut s);
            fsx::write_json(&file, &s)?;
            Ok(s)
        })
    }
}

// -------------------------------------------------------------------- account

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Identity {
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub org: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cycle {
    Monthly,
    Yearly,
}

impl Cycle {
    pub fn label(self) -> &'static str {
        match self {
            Cycle::Monthly => "monthly",
            Cycle::Yearly => "yearly",
        }
    }
}

/// The subscription behind an account. No vendor reports all of this, so the
/// parts a fetch cannot fill are typed in by hand and kept.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Billing {
    pub cycle: Option<Cycle>,
    /// A renewal date. When it has passed and the plan renews, the next one is
    /// worked out from the cycle.
    pub renews_on: Option<NaiveDate>,
    /// False for a cancelled plan: `renews_on` is then the day it ends.
    pub cancelled: bool,
    pub source: Source,
}

impl Billing {
    /// The next renewal (or end) date on or after `today`.
    pub fn next(&self, today: NaiveDate) -> Option<NaiveDate> {
        let mut date = self.renews_on?;
        if self.cancelled {
            return Some(date);
        }
        let Some(cycle) = self.cycle else {
            return (date >= today).then_some(date);
        };
        let step = match cycle {
            Cycle::Monthly => Months::new(1),
            Cycle::Yearly => Months::new(12),
        };
        // Step from the original date each time, so the 31st does not drift to
        // the 28th for the rest of the year after one February.
        let anchor = date;
        let mut n = 0u32;
        while date < today && n < 2400 {
            n += 1;
            date = anchor.checked_add_months(Months::new(step.as_u32() * n))?;
        }
        Some(date)
    }

    /// Has a cancelled plan already run out?
    pub fn ended(&self, today: NaiveDate) -> bool {
        self.cancelled && self.renews_on.is_some_and(|d| d < today)
    }
}

/// The outcome of the last usage fetch, kept so every other part of cswap can
/// work offline.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Refresh {
    pub last_attempt: Option<DateTime<Utc>>,
    pub last_success: Option<DateTime<Utc>>,
    pub error: Option<String>,
    /// The last failure needs the user (an expired or rejected login). The
    /// dashboard stops polling until the login changes.
    pub needs_user: bool,
    /// Do not ask again before this time (set after a rate limit).
    pub retry_at: Option<DateTime<Utc>>,
    pub data: Option<Snapshot>,
}

/// The outcome of the last link sync for this account.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncState {
    pub last_synced: Option<DateTime<Utc>>,
    pub last_checked: Option<DateTime<Utc>>,
    pub linked: usize,
    pub private: usize,
    /// Names where a real file sits in the way of a link.
    pub shadowed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountMeta {
    pub schema: u32,
    pub harness: String,
    /// 0 is the harness's own native login; cswap accounts count from 1.
    pub id: u32,
    pub label: String,
    pub aliases: Vec<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub last_used: Option<DateTime<Utc>>,
    pub identity: Identity,
    pub plan: Option<String>,
    pub plan_source: Source,
    pub billing: Billing,
    /// Resets entered by hand. Ones the API reports live in `refresh.data`.
    pub grants: Vec<Grant>,
    pub refresh: Refresh,
    pub sync: SyncState,
}

impl Default for AccountMeta {
    fn default() -> Self {
        AccountMeta {
            schema: SCHEMA,
            harness: String::new(),
            id: 0,
            label: String::new(),
            aliases: Vec::new(),
            created_at: None,
            last_used: None,
            identity: Identity::default(),
            plan: None,
            plan_source: Source::Api,
            billing: Billing::default(),
            grants: Vec::new(),
            refresh: Refresh::default(),
            sync: SyncState::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Account {
    pub harness: Harness,
    pub meta: AccountMeta,
}

pub fn key_of(h: Harness, id: u32) -> String {
    format!("{}-{id}", h.id())
}

impl Account {
    pub fn key(&self) -> String {
        key_of(self.harness, self.meta.id)
    }

    pub fn is_native(&self) -> bool {
        self.meta.id == 0
    }

    pub fn dir(&self) -> PathBuf {
        paths::account_dir(&self.key())
    }

    /// The directory the harness reads: the native one for account 0, the
    /// account's own `config_dir` otherwise.
    pub fn config_dir(&self) -> PathBuf {
        if self.is_native() {
            self.harness.native_dir()
        } else {
            self.dir().join("config_dir")
        }
    }

    /// What to call this account in a list: its label, else its email, else
    /// its key.
    pub fn title(&self) -> String {
        if !self.meta.label.is_empty() {
            self.meta.label.clone()
        } else if self.is_native() {
            "native".to_string()
        } else if let Some(e) = &self.meta.identity.email {
            e.clone()
        } else {
            self.key()
        }
    }

    pub fn email(&self) -> &str {
        self.meta.identity.email.as_deref().unwrap_or("")
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.meta.refresh.data.as_ref()
    }

    /// Every unexpired reset this account can still spend: the ones entered by
    /// hand and the ones the API reported.
    pub fn grants(&self, now: DateTime<Utc>) -> Vec<Grant> {
        let api = self
            .snapshot()
            .map(|s| s.grants.clone())
            .unwrap_or_default();
        self.meta
            .grants
            .iter()
            .cloned()
            .chain(api)
            .filter(|g| !g.expired(now) && g.count > 0)
            .collect()
    }

    /// Does `sel` name this account? Matches the id, the key, the label, an
    /// alias or the email, ignoring case.
    pub fn matches(&self, sel: &str) -> bool {
        let sel = sel.trim().to_lowercase();
        if sel.is_empty() {
            return false;
        }
        if self.is_native() && matches!(sel.as_str(), "native" | "default") {
            return true;
        }
        sel == self.meta.id.to_string()
            || sel == self.key()
            || (!self.meta.label.is_empty() && sel == self.meta.label.to_lowercase())
            || self.meta.aliases.iter().any(|a| a.to_lowercase() == sel)
            || self.email().to_lowercase() == sel
    }

    fn file(&self) -> PathBuf {
        self.dir().join("settings.json")
    }

    /// Re-read this account from disk, change it and write it back, all under
    /// the account's lock. `self` is replaced with what was written.
    pub fn update(&mut self, f: impl FnOnce(&mut AccountMeta)) -> Result<()> {
        let (dir, file, harness, id) = (self.dir(), self.file(), self.harness, self.meta.id);
        // A cswap account lives in its directory. If that is gone the account
        // was removed, perhaps while a fetch for it was still in flight, and
        // writing now would bring an empty shell of it back. (The native
        // login has no directory until something is stored about it.)
        if !self.is_native() && !dir.is_dir() {
            anyhow::bail!("{} no longer exists", self.key());
        }
        let meta = fsx::with_lock(&dir, || {
            let mut meta: AccountMeta = fsx::read_json(&file)?.unwrap_or_else(|| self.meta.clone());
            f(&mut meta);
            meta.schema = SCHEMA;
            meta.harness = harness.id().to_string();
            meta.id = id;
            fsx::write_json(&file, &meta)?;
            Ok(meta)
        })?;
        self.meta = meta;
        Ok(())
    }
}

/// A label has to be typeable after `--cswap=`, and must not read as an id or
/// a reserved word.
pub fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 32
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !label.chars().all(|c| c.is_ascii_digit())
        && !matches!(
            label.to_lowercase().as_str(),
            "native" | "default" | "auto" | "select"
        )
}

// ------------------------------------------------------------------- registry

/// Every account cswap knows, native ones included, ordered by harness then id.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub accounts: Vec<Account>,
}

impl Registry {
    pub fn load() -> Result<Registry> {
        let mut accounts = Vec::new();
        if let Ok(rd) = fs::read_dir(paths::accounts_dir()) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let Some((h, id)) = parse_key(&name) else {
                    continue;
                };
                let file = entry.path().join("settings.json");
                let Some(mut meta) = fsx::read_json::<AccountMeta>(&file)
                    .with_context(|| format!("account {name}"))?
                else {
                    continue;
                };
                meta.harness = h.id().to_string();
                meta.id = id;
                accounts.push(Account { harness: h, meta });
            }
        }
        // The native login of each installed harness is account 0, whether or
        // not cswap has stored anything about it yet.
        for h in Harness::ALL {
            let known = accounts.iter().any(|a| a.harness == h && a.is_native());
            if !known && h.native_dir().is_dir() {
                accounts.push(Account {
                    harness: h,
                    meta: AccountMeta {
                        harness: h.id().to_string(),
                        ..AccountMeta::default()
                    },
                });
            }
        }
        accounts.sort_by_key(|a| (a.harness.order(), a.meta.id));
        Ok(Registry { accounts })
    }

    pub fn of(&self, h: Harness) -> impl Iterator<Item = &Account> {
        self.accounts.iter().filter(move |a| a.harness == h)
    }

    pub fn get(&self, key: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.key() == key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Account> {
        self.accounts.iter_mut().find(|a| a.key() == key)
    }

    /// Resolve what the user typed to one account of `h`.
    pub fn find(&self, h: Harness, sel: &str) -> Result<&Account> {
        let hits: Vec<&Account> = self.of(h).filter(|a| a.matches(sel)).collect();
        match hits.as_slice() {
            [one] => Ok(one),
            [] => {
                let known: Vec<String> = self
                    .of(h)
                    .map(|a| format!("{} ({})", a.meta.id, a.title()))
                    .collect();
                bail!(
                    "no {} account matches '{sel}'. Known: {}",
                    h.name(),
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                )
            }
            // An exact id always wins over a label or email that also matches.
            many => many
                .iter()
                .find(|a| a.meta.id.to_string() == sel.trim())
                .copied()
                .with_context(|| format!("'{sel}' matches more than one {} account", h.name())),
        }
    }

    /// Resolve `<harness>-<id>`, or a selector qualified as `<harness>:<sel>`,
    /// or a bare selector that matches exactly one account across harnesses.
    pub fn find_any(&self, text: &str) -> Result<&Account> {
        if let Some(acc) = self.get(text) {
            return Ok(acc);
        }
        if let Some((h, sel)) = text.split_once(':') {
            if let Some(h) = Harness::from_id(h) {
                return self.find(h, sel);
            }
        }
        let hits: Vec<&Account> = self.accounts.iter().filter(|a| a.matches(text)).collect();
        match hits.as_slice() {
            [one] => Ok(one),
            [] => bail!("no account matches '{text}' (see `cswap list`)"),
            _ => bail!(
                "'{text}' matches several accounts; name one as <harness>-<id>, such as {}",
                hits[0].key()
            ),
        }
    }

    pub fn next_id(&self, h: Harness) -> u32 {
        self.of(h).map(|a| a.meta.id).max().unwrap_or(0) + 1
    }

    /// Is `label` free within `h` (ignoring the account `except`)?
    pub fn label_free(&self, h: Harness, label: &str, except: Option<u32>) -> bool {
        !self
            .of(h)
            .filter(|a| Some(a.meta.id) != except)
            .any(|a| a.matches(label))
    }
}

pub fn parse_key(key: &str) -> Option<(Harness, u32)> {
    let (h, id) = key.rsplit_once('-')?;
    Some((Harness::from_id(h)?, id.parse().ok()?))
}

/// Today's date in the local timezone.
pub fn today() -> NaiveDate {
    let now = chrono::Local::now();
    NaiveDate::from_ymd_opt(now.year(), now.month(), now.day()).expect("valid date")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn monthly_renewal_rolls_forward_without_drifting() {
        let b = Billing {
            cycle: Some(Cycle::Monthly),
            renews_on: Some(d(2026, 1, 31)),
            ..Billing::default()
        };
        assert_eq!(b.next(d(2026, 2, 10)), Some(d(2026, 2, 28)));
        // Still anchored on the 31st after February.
        assert_eq!(b.next(d(2026, 3, 1)), Some(d(2026, 3, 31)));
        assert_eq!(b.next(d(2026, 1, 31)), Some(d(2026, 1, 31)));
    }

    #[test]
    fn yearly_renewal_and_cancelled_plans() {
        let mut b = Billing {
            cycle: Some(Cycle::Yearly),
            renews_on: Some(d(2025, 6, 15)),
            ..Billing::default()
        };
        assert_eq!(b.next(d(2026, 11, 11)), Some(d(2027, 6, 15)));
        b.cancelled = true;
        assert_eq!(b.next(d(2026, 11, 11)), Some(d(2025, 6, 15)));
        assert!(b.ended(d(2026, 11, 11)));
    }

    #[test]
    fn labels_cannot_look_like_ids_or_reserved_words() {
        assert!(valid_label("work"));
        assert!(valid_label("max-20x"));
        assert!(!valid_label("2"));
        assert!(!valid_label("auto"));
        assert!(!valid_label("Native"));
        assert!(!valid_label("has space"));
    }

    #[test]
    fn keys_parse_back() {
        assert_eq!(parse_key("claude-2"), Some((Harness::Claude, 2)));
        assert_eq!(parse_key("nope-2"), None);
        assert_eq!(parse_key("claude-x"), None);
    }
}
