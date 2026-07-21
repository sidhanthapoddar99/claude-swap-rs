//! Every filesystem location cswap knows about.
//!
//! Read-side (Claude Code's own state — cswap NEVER writes here):
//!   ~/.claude/               the live config dir
//!   ~/.claude.json           identity + per-project state (next to, not inside!)
//!   ~/.claude/.credentials.json
//!
//! Write-side (cswap-owned):
//!   ~/.config/cswap/config.toml            accounts + default
//!   ~/.local/share/cswap/accounts/         stored credential/meta backups
//!   ~/.local/share/cswap/profiles/<name>/  per-account CLAUDE_CONFIG_DIR

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

pub fn accounts_dir() -> PathBuf {
    data_dir().join("accounts")
}

pub fn profiles_dir() -> PathBuf {
    data_dir().join("profiles")
}

pub fn profile_dir(name: &str) -> PathBuf {
    profiles_dir().join(name)
}

pub fn store_creds(name: &str) -> PathBuf {
    accounts_dir().join(format!("{name}.creds.json"))
}

pub fn store_meta(name: &str) -> PathBuf {
    accounts_dir().join(format!("{name}.meta.json"))
}
