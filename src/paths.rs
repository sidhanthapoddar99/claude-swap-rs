//! Every filesystem location cswap knows about.
//!
//! Read-side (the `default` entity — cswap NEVER writes here):
//!   ~/.claude/               the live config dir
//!   ~/.claude.json           identity + per-project state (next to, not inside!)
//!   ~/.claude/.credentials.json
//!
//! Write-side (cswap-owned):
//!   ~/.config/cswap/config.toml             the profile registry
//!   ~/.local/share/cswap/profiles/<email>/  one directory per profile
//!
//! A profile directory is the whole entity: its own `.credentials.json` is the
//! only copy of that profile's tokens. There is no separate credential store —
//! a second copy is how one account ends up with two refresh-token families.

use std::path::PathBuf;

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is not set"))
}

pub fn claude_dir() -> PathBuf {
    home().join(".claude")
}

/// The global config file. NOTE: with CLAUDE_CONFIG_DIR *unset* this lives in
/// $HOME, not inside ~/.claude; with it set, Claude looks inside the dir.
pub fn claude_json() -> PathBuf {
    home().join(".claude.json")
}

pub fn live_credentials() -> PathBuf {
    claude_dir().join(".credentials.json")
}

fn xdg(var: &str, fallback: &[&str]) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            let mut p = home();
            for seg in fallback {
                p.push(seg);
            }
            p
        }
    }
}

pub fn config_file() -> PathBuf {
    xdg("XDG_CONFIG_HOME", &[".config"])
        .join("cswap")
        .join("config.toml")
}

pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", &[".local", "share"]).join("cswap")
}

pub fn profiles_dir() -> PathBuf {
    data_dir().join("profiles")
}

/// A profile's directory, keyed by its account. One profile per account, so
/// the email is unique and the directory never has to move. That stability
/// matters: claude records absolute paths into ~/.claude (plugin install
/// locations, session aliases) that would dangle if a profile were renamed.
pub fn profile_dir(email: &str) -> PathBuf {
    profiles_dir().join(email)
}

/// Pre-0.6 credential store. Kept only so the migration can move it aside.
pub fn legacy_accounts_dir() -> PathBuf {
    data_dir().join("accounts")
}
