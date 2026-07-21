//! ~/.config/cswap/config.toml — the account registry.
//!
//! ```toml
//! default = "personal"
//!
//! [[account]]
//! name = "personal"
//! email = "you@gmail.com"
//!
//! [[account]]
//! name = "work"
//! email = "you@corp.com"
//! isolated = true   # own projects/ + history.jsonl (no shared history)
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

use crate::paths;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default, rename = "account")]
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub isolated: bool,
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.')
        && name != "default"
        && name != "off"
}

impl Config {
    pub fn load() -> Result<Config> {
        let path = paths::config_file();
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("malformed config: {}", path.display()))
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

    /// Look up by name first, then by email.
    pub fn find(&self, key: &str) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|a| a.name == key)
            .or_else(|| self.accounts.iter().find(|a| a.email == key))
    }

    /// The account a bare `claude` should run as: $CSWAP_ACTIVE > default.
    pub fn resolve_active(&self) -> Result<&Account> {
        if let Ok(name) = std::env::var("CSWAP_ACTIVE") {
            if !name.is_empty() {
                return self.find(&name).with_context(|| {
                    format!("CSWAP_ACTIVE={name} does not match any account (see `cswap list`)")
                });
            }
        }
        let default = self.default.as_deref().context(
            "no account active and no default set — run `cswap login`, then `cswap default <name>`",
        )?;
        self.find(default)
            .with_context(|| format!("default account '{default}' not found in config"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(valid_name("dev"));
        assert!(valid_name("work-2"));
        assert!(valid_name("a_b.c"));
        assert!(!valid_name(""));
        assert!(!valid_name("Dev"));
        assert!(!valid_name("has space"));
        assert!(!valid_name(".hidden"));
        assert!(!valid_name("default"));
        assert!(!valid_name("off"));
    }

    #[test]
    fn find_by_name_and_email() {
        let cfg = Config {
            default: Some("a".into()),
            accounts: vec![
                Account {
                    name: "a".into(),
                    email: "a@x.com".into(),
                    isolated: false,
                },
                Account {
                    name: "b".into(),
                    email: "b@x.com".into(),
                    isolated: true,
                },
            ],
        };
        assert_eq!(cfg.find("b").unwrap().email, "b@x.com");
        assert_eq!(cfg.find("a@x.com").unwrap().name, "a");
        assert!(cfg.find("zzz").is_none());
    }

    #[test]
    fn toml_roundtrip() {
        let cfg = Config {
            default: Some("dev".into()),
            accounts: vec![Account {
                name: "dev".into(),
                email: "d@x.com".into(),
                isolated: false,
            }],
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.default.as_deref(), Some("dev"));
        assert_eq!(back.accounts.len(), 1);
    }
}
