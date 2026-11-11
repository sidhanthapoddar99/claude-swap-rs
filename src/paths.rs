//! Every filesystem location cswap owns.
//!
//! ```text
//! ~/.cswap/
//!   settings.json                    global settings
//!   harness/<harness>/settings.json  per-harness settings: default account,
//!                                    wrapping, sync rules and last sync check
//!   accounts/<harness>-<id>/
//!     settings.json                  the account: label, identity, plan,
//!                                    billing, last refresh and its data
//!     config_dir/                    what the harness is pointed at: this
//!                                    account's credentials, and links to the
//!                                    native config dir for everything else
//!   history/<harness>-<id>.jsonl     usage samples over time
//!   parked/                          files that shadowed a link, moved aside
//! ```
//!
//! The native config dirs (`~/.claude`, `~/.codex`, …) are read, never written.

use std::path::PathBuf;

pub fn home() -> PathBuf {
    crate::platform::home().expect("cannot find your home directory: HOME is not set")
}

/// cswap's own root. `CSWAP_HOME` moves it, which is what the tests use.
pub fn root() -> PathBuf {
    match std::env::var_os("CSWAP_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(".cswap"),
    }
}

pub fn settings_file() -> PathBuf {
    root().join("settings.json")
}

pub fn harness_settings(harness_id: &str) -> PathBuf {
    root()
        .join("harness")
        .join(harness_id)
        .join("settings.json")
}

pub fn accounts_dir() -> PathBuf {
    root().join("accounts")
}

/// `key` is `<harness>-<id>`, such as `claude-2`.
pub fn account_dir(key: &str) -> PathBuf {
    accounts_dir().join(key)
}

pub fn history_file(key: &str) -> PathBuf {
    root().join("history").join(format!("{key}.jsonl"))
}

pub fn parked_dir(key: &str) -> PathBuf {
    root().join("parked").join(key)
}

pub fn cache_dir() -> PathBuf {
    root().join("cache")
}

/// Shorten a path under `$HOME` to `~/…` for display.
pub fn tilde(path: &std::path::Path) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}
