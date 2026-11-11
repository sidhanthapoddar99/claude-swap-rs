//! The coding agents cswap manages, and the one interface each implements.
//!
//! A harness is described by a `Spec` (where it keeps state, which env var
//! moves that, which names are private to an account) and four operations:
//! is a login present, who is it, what is its usage, and how to log in. Each
//! vendor module holds exactly what was verified against that vendor's CLI;
//! nothing here guesses across vendors.

use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

use crate::http::Fetch;
use crate::model::Snapshot;
use crate::paths;
use crate::store::{Billing, Identity};

pub mod agy;
pub mod claude;
pub mod codex;
pub mod grok;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Harness {
    Claude,
    Codex,
    Agy,
    Grok,
}

pub struct Spec {
    /// Used in directory names and on the command line: `claude`.
    pub id: &'static str,
    pub name: &'static str,
    /// The vendor's name for the limits page, for messages.
    pub vendor: &'static str,
    /// Binary names, most common first. The first one found on `PATH` runs.
    pub bins: &'static [&'static str],
    /// The env var that moves the whole config dir.
    pub home_env: Option<&'static str>,
    /// A command-line flag that moves the config dir, for an agent with no
    /// env var for it. The directory is appended: `--gemini_dir=` + path.
    pub home_flag: Option<&'static str>,
    /// Directories mirrored one level deeper instead of linked whole, because
    /// they hold both shared and private entries. Paths relative to the dir.
    pub nested: &'static [&'static str],
    /// The agent's support in cswap is new and not fully verified.
    pub experimental: bool,
    /// The native config dir, relative to `$HOME`.
    pub native: &'static str,
    /// Names in the config dir that belong to one account and are never
    /// linked: credentials, identity, per-account caches. A leading or trailing
    /// `*` matches any suffix or prefix.
    pub private: &'static [&'static str],
    /// The file that holds the login. Its modification time tells cswap when
    /// a login changed, so an account that needed the user is asked about again.
    pub cred_file: &'static str,
    /// What is known about how long a login lasts, for the accounts page.
    pub login_life: &'static str,
    /// Env vars that would make the agent ignore the account's login. Removed
    /// from the environment of a launch on a cswap account.
    pub scrub_env: &'static [&'static str],
    /// Arguments that start the agent's own login flow.
    pub login_args: &'static [&'static str],
    /// Accent colour in the dashboard.
    pub rgb: (u8, u8, u8),
}

/// What is known about a login from disk alone.
#[derive(Debug, Clone, Default)]
pub struct Local {
    pub identity: Identity,
    pub plan: Option<String>,
    /// When the stored access token stops working, if the file says.
    pub token_expires: Option<DateTime<Utc>>,
    /// When the login itself runs out and the user must sign in again, for
    /// the vendors that say.
    pub login_expires: Option<DateTime<Utc>>,
    pub billing: Option<Billing>,
}

/// What a usage fetch returns: the snapshot, plus anything the same call
/// revealed about who the account is.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub snapshot: Snapshot,
    pub local: Local,
}

impl Harness {
    pub const ALL: [Harness; 4] = [Harness::Claude, Harness::Codex, Harness::Agy, Harness::Grok];

    pub fn spec(self) -> &'static Spec {
        match self {
            Harness::Claude => &claude::SPEC,
            Harness::Codex => &codex::SPEC,
            Harness::Agy => &agy::SPEC,
            Harness::Grok => &grok::SPEC,
        }
    }

    pub fn id(self) -> &'static str {
        self.spec().id
    }

    pub fn name(self) -> &'static str {
        self.spec().name
    }

    pub fn order(self) -> usize {
        Harness::ALL.iter().position(|h| *h == self).unwrap_or(0)
    }

    pub fn from_id(id: &str) -> Option<Harness> {
        let id = id.to_lowercase();
        Harness::ALL
            .into_iter()
            .find(|h| h.id() == id || h.spec().bins.contains(&id.as_str()))
            .or(matches!(id.as_str(), "antigravity" | "gemini").then_some(Harness::Agy))
    }

    /// The agent's own config dir. `CSWAP_NATIVE_<ID>` moves it, for tests.
    pub fn native_dir(self) -> PathBuf {
        let var = format!("CSWAP_NATIVE_{}", self.id().to_uppercase());
        match std::env::var_os(var) {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => paths::home().join(self.spec().native),
        }
    }

    /// Is `name` private to an account for this harness?
    pub fn is_private(self, name: &str) -> bool {
        self.spec().private.iter().any(|pat| {
            if let Some(suffix) = pat.strip_prefix('*') {
                name.ends_with(suffix)
            } else if let Some(prefix) = pat.strip_suffix('*') {
                name.starts_with(prefix)
            } else {
                *pat == name
            }
        })
    }

    /// The environment a launch on `config_dir` needs, beyond the config dir
    /// variable itself.
    pub fn launch_env(self) -> Vec<(&'static str, std::ffi::OsString)> {
        match self {
            // Every account reads the one set of databases in the native dir.
            // This is Codex's own setting for it; SQLite files are never
            // shared by link.
            Harness::Codex => vec![("CODEX_SQLITE_HOME", self.native_dir().into_os_string())],
            Harness::Agy => agy::account_env(),
            _ => Vec::new(),
        }
    }

    /// Arguments that point the agent at `config_dir`, for an agent moved by a
    /// flag. They go before everything the user typed.
    pub fn launch_args(self, config_dir: &Path) -> Vec<std::ffi::OsString> {
        match self.spec().home_flag {
            Some(flag) => {
                let mut arg = std::ffi::OsString::from(flag);
                arg.push(config_dir.as_os_str());
                vec![arg]
            }
            None => Vec::new(),
        }
    }

    /// The real binary: `CSWAP_<ID>_BIN`, else the path from the settings,
    /// else the first of the agent's names on `PATH` that is not cswap itself.
    pub fn binary(self, configured: Option<&str>) -> anyhow::Result<PathBuf> {
        use anyhow::Context;
        let var = format!("CSWAP_{}_BIN", self.id().to_uppercase());
        let from_env = std::env::var(&var).ok();
        for (source, value) in [
            (var.as_str(), from_env.as_deref()),
            ("settings", configured),
        ] {
            if let Some(v) = value.filter(|v| !v.is_empty()) {
                let p = PathBuf::from(v);
                if p.is_file() {
                    return Ok(p);
                }
                anyhow::bail!(
                    "the {} binary set in {source} is not a file: {v}",
                    self.name()
                );
            }
        }
        let me = std::env::current_exe()
            .ok()
            .and_then(|p| std::fs::canonicalize(p).ok());
        self.spec()
            .bins
            .iter()
            .find_map(|name| crate::fsx::which(name, me.as_deref()))
            .with_context(|| {
                format!(
                    "`{}` is not on PATH. Install {} first, or set its path in cswap settings",
                    self.spec().bins[0],
                    self.name()
                )
            })
    }

    /// Can cswap run this agent on a directory of its own choosing?
    pub fn wrappable(self) -> bool {
        self.spec().home_env.is_some() || self.spec().home_flag.is_some()
    }

    /// Is there a login in this config dir? Disk only.
    pub fn logged_in(self, config_dir: &Path) -> bool {
        let native = config_dir == self.native_dir();
        match self {
            Harness::Claude => claude::logged_in(config_dir),
            Harness::Codex => codex::logged_in(config_dir),
            Harness::Agy => agy::logged_in(config_dir, native),
            Harness::Grok => grok::logged_in(config_dir),
        }
    }

    /// Who is logged in, from disk alone.
    pub fn local(self, config_dir: &Path, native: bool) -> Local {
        match self {
            Harness::Claude => claude::local(config_dir, native),
            Harness::Codex => codex::local(config_dir),
            Harness::Agy => agy::local(config_dir, native),
            Harness::Grok => grok::local(config_dir),
        }
    }

    /// Ask the vendor for this account's usage.
    ///
    /// cswap never renews a token to do this: every one of these agents uses
    /// single-use refresh tokens or its own lock around renewal, so a second
    /// refresher can log the account out. A login whose access token has run
    /// out reports `Why::Expired` until the agent itself next runs.
    ///
    /// `prev` is the last stored snapshot, for the parts that are asked for
    /// less often than usage and carried forward in between.
    pub fn fetch(self, config_dir: &Path, native: bool, prev: Option<&Snapshot>) -> Fetch<Fetched> {
        match self {
            Harness::Claude => claude::fetch(config_dir, native, prev),
            Harness::Codex => codex::fetch(config_dir),
            Harness::Agy => agy::fetch(config_dir, native),
            Harness::Grok => grok::fetch(config_dir),
        }
    }

    /// Prepare a brand-new account dir before the agent first runs in it.
    pub fn seed(self, config_dir: &Path) -> anyhow::Result<()> {
        match self {
            Harness::Claude => claude::seed(config_dir),
            _ => Ok(()),
        }
    }

    /// Re-assert owner-only permissions on the files that hold a login. The
    /// agents rewrite them by rename, which resets the mode to their umask.
    pub fn harden(self, config_dir: &Path) {
        for name in self.spec().private.iter().filter(|n| !n.contains('*')) {
            let p = config_dir.join(name);
            if p.is_file() {
                let _ = crate::platform::make_private(&p, false);
            }
        }
    }
}

/// Has a token with this expiry run out? A minute of margin, so a call is not
/// started with a token that dies mid-flight.
pub(crate) fn expired(expires: Option<DateTime<Utc>>) -> bool {
    expires.is_some_and(|t| t <= Utc::now() + chrono::Duration::seconds(60))
}

/// Parse an RFC 3339 time into UTC.
pub(crate) fn rfc3339(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// A unix time in seconds, or in milliseconds when it is too large to be
/// seconds.
pub(crate) fn unix(n: i64) -> Option<DateTime<Utc>> {
    if n > 100_000_000_000 {
        DateTime::from_timestamp_millis(n)
    } else {
        DateTime::from_timestamp(n, 0)
    }
}

/// Tidy a vendor's plan identifier for display: `default_claude_max_20x` and
/// `prolite` become `Max 20x` and `Prolite`.
pub(crate) fn pretty_plan(raw: &str) -> String {
    let cleaned = raw
        .trim()
        .trim_start_matches("default_")
        .trim_start_matches("claude_")
        .replace(['_', '-'], " ");
    cleaned
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) if w.chars().any(|ch| ch.is_ascii_digit()) => {
                    format!("{}{}", f, c.as_str())
                }
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_read_like_names() {
        assert_eq!(pretty_plan("default_claude_max_20x"), "Max 20x");
        assert_eq!(pretty_plan("prolite"), "Prolite");
        assert_eq!(pretty_plan("SuperGrok"), "SuperGrok");
        assert_eq!(pretty_plan("pro"), "Pro");
    }

    #[test]
    fn ids_and_binary_names_both_resolve() {
        assert_eq!(Harness::from_id("claude"), Some(Harness::Claude));
        assert_eq!(Harness::from_id("CODEX"), Some(Harness::Codex));
        assert_eq!(Harness::from_id("antigravity"), Some(Harness::Agy));
        assert_eq!(Harness::from_id("nope"), None);
    }

    #[test]
    fn private_names_match_exactly_or_by_star() {
        assert!(Harness::Codex.is_private("auth.json"));
        assert!(Harness::Codex.is_private("state_5.sqlite"));
        assert!(Harness::Codex.is_private("state_5.sqlite-wal"));
        assert!(Harness::Codex.is_private("config.toml.backup-2026"));
        assert!(!Harness::Codex.is_private("config.toml"));
        assert!(!Harness::Codex.is_private("sessions"));
    }

    #[test]
    fn unix_times_in_seconds_or_millis() {
        assert_eq!(unix(1_794_607_688).unwrap().timestamp(), 1_794_607_688);
        assert_eq!(unix(1_794_607_688_000).unwrap().timestamp(), 1_794_607_688);
    }
}
