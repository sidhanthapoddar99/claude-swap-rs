//! `cswap migrate`: bring the Claude profiles of cswap 0.6 into `~/.cswap`.
//!
//! 0.6 kept one directory per profile under `~/.local/share/cswap/profiles/
//! <email>/` and listed aliases in `~/.config/cswap/config.toml`. Each becomes
//! a Claude account here. The directory is moved, not copied, so the login
//! stays a single copy; a link is left at the old path, because Claude Code
//! wrote that path into its own files and open terminals may still point at
//! it.

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

use crate::harness::Harness;
use crate::store::{self, Account, AccountMeta, HarnessSettings, Registry};
use crate::{fsx, paths, platform, refresh, sync};

fn xdg(var: &str, fallback: &[&str]) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => fallback.iter().fold(paths::home(), |p, seg| p.join(seg)),
    }
}

fn old_config() -> PathBuf {
    xdg("XDG_CONFIG_HOME", &[".config"])
        .join("cswap")
        .join("config.toml")
}

fn old_profiles() -> PathBuf {
    xdg("XDG_DATA_HOME", &[".local", "share"])
        .join("cswap")
        .join("profiles")
}

#[derive(Debug, Deserialize)]
struct RawEntry {
    email: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    name: Option<String>,
}

/// Every shape the old file ever had: `[[profile]]` and, before 0.6,
/// `[[account]]`.
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default, rename = "profile")]
    profiles: Vec<RawEntry>,
    #[serde(default, rename = "account")]
    accounts: Vec<RawEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Old {
    pub email: String,
    pub aliases: Vec<String>,
    pub dir: PathBuf,
}

/// The 0.6 profiles still waiting: a real directory with a login in it. One
/// that was already moved is a link now and is left out.
pub fn pending() -> Vec<Old> {
    let config: RawConfig = fs::read_to_string(old_config())
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default();
    let names = |email: &str| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for e in config
            .profiles
            .iter()
            .chain(&config.accounts)
            .filter(|e| e.email == email)
        {
            for a in e.name.iter().chain(&e.aliases) {
                if !a.is_empty() && !out.contains(a) {
                    out.push(a.clone());
                }
            }
        }
        out
    };
    let Ok(rd) = fs::read_dir(old_profiles()) else {
        return Vec::new();
    };
    let mut out: Vec<Old> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| e.path().join(".credentials.json").is_file())
        .map(|e| {
            let email = e.file_name().to_string_lossy().to_string();
            Old {
                aliases: names(&email),
                email,
                dir: e.path(),
            }
        })
        .collect();
    out.sort_by(|a, b| a.email.cmp(&b.email));
    out
}

/// Move one profile in. Returns the new account.
fn adopt(old: &Old) -> Result<Account> {
    let h = Harness::Claude;
    fsx::private_dir(&paths::accounts_dir())?;
    let mut acc = fsx::with_lock(&paths::accounts_dir(), || {
        let reg = Registry::load()?;
        // The first old alias that can be a label here becomes the label; the
        // rest stay as extra names.
        let usable: Vec<String> = old
            .aliases
            .iter()
            .filter(|a| store::valid_label(a) && reg.label_free(h, a, None))
            .cloned()
            .collect();
        let mut acc = Account {
            harness: h,
            meta: AccountMeta {
                harness: h.id().to_string(),
                id: reg.next_id(h),
                label: usable.first().cloned().unwrap_or_default(),
                aliases: usable.iter().skip(1).cloned().collect(),
                created_at: Some(Utc::now()),
                ..AccountMeta::default()
            },
        };
        fsx::private_dir(&acc.dir())?;
        acc.update(|_| {})?;
        Ok(acc)
    })?;

    let dest = acc.config_dir();
    if let Err(e) = fs::rename(&old.dir, &dest) {
        let _ = fs::remove_dir_all(acc.dir());
        return Err(e).with_context(|| {
            format!(
                "cannot move {} to {} (are they on different disks?)",
                old.dir.display(),
                dest.display()
            )
        });
    }
    // Old paths keep working. Not fatal if the link cannot be made.
    let _ = platform::symlink(&dest, &old.dir);

    let hs = HarnessSettings::load(h)?;
    sync::sync_account(&mut acc, &hs)?;
    h.harden(&dest);
    refresh::refresh_local(&mut acc)?;
    Ok(acc)
}

pub fn run() -> Result<()> {
    let old = pending();
    if old.is_empty() {
        println!("Nothing to migrate: no cswap 0.6 profiles were found.");
        return Ok(());
    }
    let reg = Registry::load()?;
    if !crate::sessions::scan(&reg).is_empty() {
        // 0.6 sessions are not attributed to any account here, so this only
        // catches some; the link left behind covers the rest.
        eprintln!(
            "cswap: agents are running. Sessions on an old profile keep working through a link."
        );
    }
    let mut failed = 0;
    for profile in &old {
        match adopt(profile) {
            Ok(acc) => {
                let name = if acc.meta.label.is_empty() {
                    String::new()
                } else {
                    format!(" as '{}'", acc.meta.label)
                };
                println!("✓ {} → {}{name}", profile.email, acc.key());
            }
            Err(e) => {
                failed += 1;
                eprintln!("✗ {}: {e:#}", profile.email);
            }
        }
    }
    println!();
    println!("Each login was moved, not copied. A link stays at the old path.");
    println!("See them with: cswap list");
    if failed > 0 {
        bail!("{failed} profile(s) could not be moved");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Sandbox;

    fn old_profile(email: &str, extra_toml: &str) -> PathBuf {
        let dir = old_profiles().join(email);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":4102444800000}}"#,
        )
        .unwrap();
        fs::write(
            dir.join(".claude.json"),
            format!(r#"{{"oauthAccount":{{"emailAddress":"{email}"}}}}"#),
        )
        .unwrap();
        let cfg = old_config();
        fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        let mut text = fs::read_to_string(&cfg).unwrap_or_default();
        text.push_str(extra_toml);
        fs::write(&cfg, text).unwrap();
        dir
    }

    #[test]
    fn old_profiles_become_claude_accounts_and_keep_their_names() {
        let sb = Sandbox::new();
        fs::create_dir_all(sb.native(Harness::Claude)).unwrap();
        fs::write(sb.native(Harness::Claude).join("settings.json"), "{}").unwrap();
        let dir = old_profile(
            "me@corp.com",
            "[[profile]]\nemail = \"me@corp.com\"\naliases = [\"work\", \"w\", \"default\"]\n",
        );
        old_profile("other@x.com", "");

        let found = pending();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].aliases, ["work", "w", "default"]);

        run().unwrap();
        let reg = Registry::load().unwrap();
        let work = reg.find(Harness::Claude, "work").unwrap();
        assert_eq!(work.email(), "me@corp.com");
        // `default` is reserved here, so it is not carried over.
        assert_eq!(work.meta.aliases, ["w"]);
        assert!(work.config_dir().join(".credentials.json").is_file());
        // The shared files are linked, and the old path still resolves.
        assert!(
            fs::symlink_metadata(work.config_dir().join("settings.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(fs::symlink_metadata(&dir).unwrap().file_type().is_symlink());
        assert!(dir.join(".credentials.json").is_file());
        assert!(reg.find(Harness::Claude, "other@x.com").is_ok());

        // A second run finds nothing left.
        assert!(pending().is_empty());
    }
}
