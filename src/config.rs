//! ~/.config/cswap/config.toml — the account registry.
//!
//! There is no "name": an account IS its email (the unique identity), and
//! aliases are the labels you type. Anywhere an account is referenced
//! (activate/run/default/alias/remove) an alias or the email resolves.
//!
//! There is also no stored "default": the default is DERIVED — it is whoever
//! is logged into the live ~/.claude right now (see [`crate::profile::live_email`]).
//! `cswap default <x>` swaps that live login; nothing about it is persisted
//! here. Pre-0.4 configs carried a `default = "..."` line; it is ignored on
//! load and stripped on the next rewrite.
//!
//! ```toml
//! [[account]]
//! email = "you@gmail.com"
//! aliases = ["personal", "p"]
//!
//! [[account]]
//! email = "you@corp.com"
//! aliases = ["work"]
//! isolated = true   # own projects/ + history.jsonl (no shared history)
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

use crate::{paths, profile};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, rename = "account")]
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub email: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub isolated: bool,
    /// Legacy (pre-0.4) primary name — migrated into aliases on load,
    /// never written back.
    #[serde(default, skip_serializing)]
    name: Option<String>,
}

impl Account {
    pub fn new(email: String) -> Account {
        Account {
            email,
            aliases: Vec::new(),
            isolated: false,
            name: None,
        }
    }

    /// What we show for this account: first alias, else the email.
    pub fn label(&self) -> &str {
        self.aliases
            .first()
            .map(String::as_str)
            .unwrap_or(&self.email)
    }

    pub fn matches(&self, key: &str) -> bool {
        self.email == key || self.aliases.iter().any(|a| a == key)
    }
}

pub fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 64
        && label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && !label.starts_with('.')
        && label != "default"
        && label != "off"
}

impl Config {
    pub fn load() -> Result<Config> {
        let path = paths::config_file();
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&text)
            .with_context(|| format!("malformed config: {}", path.display()))?;
        cfg.migrate_legacy_names();
        Ok(cfg)
    }

    /// Pre-0.4 configs had `name = "..."`: fold it in as the primary alias.
    /// Returns true when anything was migrated (caller may persist).
    pub fn migrate_legacy_names(&mut self) -> bool {
        let mut changed = false;
        for acct in &mut self.accounts {
            if let Some(name) = acct.name.take() {
                if !acct.aliases.contains(&name) {
                    acct.aliases.insert(0, name);
                }
                changed = true;
            }
        }
        changed
    }

    pub fn save(&self) -> Result<()> {
        let path = paths::config_file();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    /// Resolve an alias or email to an account.
    pub fn find(&self, key: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.matches(key))
    }

    /// Is this label already used as any account's alias (or an email)?
    pub fn label_taken(&self, label: &str) -> bool {
        self.accounts
            .iter()
            .any(|a| a.email == label || a.aliases.iter().any(|al| al == label))
    }

    /// The account a bare `claude` should run as: $CSWAP_ACTIVE if this shell
    /// activated one, otherwise the default (= the live ~/.claude login).
    ///
    /// The returned account is owned because the default may not be registered
    /// — when the live login has no config entry we synthesise a bare account
    /// for it, which `run` launches via passthrough against ~/.claude.
    pub fn resolve_active(&self) -> Result<Account> {
        if let Ok(key) = std::env::var("CSWAP_ACTIVE") {
            if !key.is_empty() {
                return self.find(&key).cloned().with_context(|| {
                    format!("CSWAP_ACTIVE={key} does not match any account (see `cswap list`)")
                });
            }
        }
        let email = profile::live_email().context(
            "nothing activated and no live ~/.claude login — run `claude` and log in, \
             or `cswap activate <alias|email>`",
        )?;
        Ok(self
            .find(&email)
            .cloned()
            .unwrap_or_else(|| Account::new(email)))
    }
}

/// One-time on-disk migration from the pre-0.4 name-keyed layout: store files
/// (`accounts/<name>.*`) and profile dirs (`profiles/<name>`) move to their
/// email-keyed paths, and the config is rewritten without `name`.
pub fn migrate_on_disk() -> Result<()> {
    let path = paths::config_file();
    if !path.exists() {
        return Ok(());
    }
    let text = fs::read_to_string(&path)?;
    let mut cfg: Config = match toml::from_str(&text) {
        Ok(c) => c,
        Err(_) => return Ok(()), // load() will surface the real error later
    };
    // Detect legacy names BEFORE migrate_legacy_names() consumes them.
    let legacy: Vec<(String, String)> = cfg
        .accounts
        .iter()
        .filter_map(|a| a.name.clone().map(|n| (n, a.email.clone())))
        .collect();
    let names_migrated = cfg.migrate_legacy_names();
    // Pre-0.4 configs carry a top-level `default = "..."` that the model no
    // longer stores; a rewrite drops it since Config has no such field.
    let has_stale_default = text
        .lines()
        .any(|l| l.trim_start().starts_with("default ="));
    if !names_migrated && !has_stale_default {
        return Ok(());
    }
    for (old, email) in legacy {
        for (from, to) in [
            (paths::store_creds(&old), paths::store_creds(&email)),
            (paths::store_meta(&old), paths::store_meta(&email)),
            (paths::profile_dir(&old), paths::profile_dir(&email)),
        ] {
            if from.exists() && !to.exists() {
                let _ = fs::rename(&from, &to);
            }
        }
    }
    cfg.save()?;
    let what = if names_migrated {
        "the email-keyed layout (aliases replace names)"
    } else {
        "the derived-default layout (the default is now the live ~/.claude login)"
    };
    eprintln!("cswap: migrated config to {what}.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(email: &str, aliases: &[&str]) -> Account {
        Account {
            email: email.into(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            isolated: false,
            name: None,
        }
    }

    #[test]
    fn label_validation() {
        assert!(valid_label("dev"));
        assert!(valid_label("work-2"));
        assert!(!valid_label(""));
        assert!(!valid_label("Dev"));
        assert!(!valid_label("has space"));
        assert!(!valid_label(".hidden"));
        assert!(!valid_label("default"));
        assert!(!valid_label("off"));
    }

    #[test]
    fn find_by_alias_and_email_and_labels() {
        let cfg = Config {
            accounts: vec![acct("a@x.com", &["alpha", "a1"]), acct("b@x.com", &[])],
        };
        assert_eq!(cfg.find("alpha").unwrap().email, "a@x.com");
        assert_eq!(cfg.find("a1").unwrap().email, "a@x.com");
        assert_eq!(cfg.find("b@x.com").unwrap().email, "b@x.com");
        assert!(cfg.find("zzz").is_none());
        assert!(cfg.label_taken("alpha"));
        assert!(cfg.label_taken("a@x.com"));
        assert!(!cfg.label_taken("free"));
        assert_eq!(cfg.accounts[0].label(), "alpha");
        assert_eq!(cfg.accounts[1].label(), "b@x.com");
    }

    #[test]
    fn legacy_name_migrates_to_alias_and_default_is_ignored() {
        let text = "default = \"main\"\n\n[[account]]\nname = \"main\"\nemail = \"m@x.com\"\n";
        let mut cfg: Config = toml::from_str(text).unwrap();
        assert!(cfg.migrate_legacy_names());
        assert_eq!(cfg.accounts[0].aliases, vec!["main".to_string()]);
        // Re-serialization carries neither a name nor a default field.
        let out = toml::to_string_pretty(&cfg).unwrap();
        assert!(!out.contains("name ="));
        assert!(!out.contains("default ="), "default is no longer stored");
        assert!(out.contains("aliases = [\"main\"]"));
    }

    #[test]
    fn toml_roundtrip() {
        let cfg = Config {
            accounts: vec![acct("d@x.com", &["dev"])],
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.accounts[0].aliases, vec!["dev".to_string()]);
        // A stray legacy `default =` deserializes fine and is dropped.
        let back2: Config = toml::from_str(&format!("default = \"d@x.com\"\n{text}")).unwrap();
        assert_eq!(back2.accounts.len(), 1);
    }
}
