//! ~/.config/cswap/config.toml — the profile registry.
//!
//! A profile is a whole entity: its own directory, its own login, its own OAuth
//! token family. It IS an account — the email is the key, so no two profiles
//! can hold the same account. Aliases are the labels you type.
//!
//! `default` is not in here and never can be: it means the live `~/.claude`,
//! which cswap only ever reads. It is the ONE thing allowed to hold the same
//! account as a profile, because it is not a profile.
//!
//! ```toml
//! [[profile]]
//! email = "you@gmail.com"
//! aliases = ["personal", "p"]
//!
//! [[profile]]
//! email = "you@corp.com"
//! aliases = ["work"]
//! ```

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

use crate::paths;

/// The reserved word that selects the live ~/.claude instead of a profile.
pub const DEFAULT_KEY: &str = "default";

/// What gets written back. Only ever serialized.
#[derive(Debug, Default, Serialize)]
pub struct Config {
    #[serde(rename = "profile")]
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Profile {
    /// The key: identifies the entity, names its directory. One per account.
    pub email: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// Every historical entry shape, read permissively:
///   0.5    `[[account]]` with email + aliases
///   0.6.0  `[[profile]]` with a separate `name` key
///   pre-0.4 `name` as the primary label
#[derive(Debug, Clone, Deserialize)]
struct RawEntry {
    email: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default, rename = "profile")]
    profiles: Vec<RawEntry>,
    #[serde(default, rename = "account")]
    accounts: Vec<RawEntry>,
}

impl RawEntry {
    /// Fold any historical `name` into the alias list; the email is the key.
    fn normalize(&self) -> Profile {
        let mut aliases = Vec::new();
        if let Some(n) = self.name.as_ref().filter(|n| !n.is_empty()) {
            aliases.push(n.clone());
        }
        for a in &self.aliases {
            if !aliases.contains(a) {
                aliases.push(a.clone());
            }
        }
        Profile {
            email: self.email.clone(),
            aliases,
        }
    }
}

impl Profile {
    pub fn new(email: String) -> Profile {
        Profile {
            email,
            aliases: Vec::new(),
        }
    }

    /// What we show for this profile: first alias, else the email.
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

/// What a launch runs as. `Default` is the live ~/.claude and owns no cswap
/// state; `Profile` is one of the registered entities.
#[derive(Debug, Clone)]
pub enum Target {
    Default,
    Profile(Profile),
}

impl Target {
    /// A stable key for this target: the email, or the reserved word.
    pub fn key(&self) -> &str {
        match self {
            Target::Default => DEFAULT_KEY,
            Target::Profile(p) => &p.email,
        }
    }
}

pub fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 64
        && label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && !label.starts_with('.')
        && !is_default_key(label)
}

/// An email has to be usable as a directory name and distinguishable from the
/// reserved words. Deliberately loose otherwise — Anthropic decides what a
/// valid address is, not us.
pub fn valid_account(email: &str) -> bool {
    !email.is_empty()
        && email.len() <= 254
        && email.contains('@')
        && !email.contains('/')
        && !email.contains('\\')
        && !email.starts_with('.')
        && !email.contains(char::is_whitespace)
        && !is_default_key(email)
}

pub fn is_default_key(key: &str) -> bool {
    key == DEFAULT_KEY || key == "off"
}

impl Config {
    pub fn load() -> Result<Config> {
        let path = paths::config_file();
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let raw: RawConfig = toml::from_str(&text)
            .with_context(|| format!("malformed config: {}", path.display()))?;
        // Either key is accepted on load, so a config the migration hasn't
        // rewritten yet still works.
        Ok(Config {
            profiles: raw
                .profiles
                .iter()
                .chain(raw.accounts.iter())
                .map(RawEntry::normalize)
                .collect(),
        })
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

    /// Resolve an email or alias to a profile. `default` never resolves here —
    /// callers that accept it must check [`is_default_key`] first.
    pub fn find(&self, key: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.matches(key))
    }

    /// Is this label already an email or alias in use?
    pub fn label_taken(&self, label: &str) -> bool {
        self.profiles
            .iter()
            .any(|p| p.email == label || p.aliases.iter().any(|a| a == label))
    }

    /// What a bare `claude` runs as: $CSWAP_ACTIVE if this shell activated a
    /// profile, otherwise `default` (the live ~/.claude).
    ///
    /// Note what is NOT here: no comparison against the live login. A profile
    /// is never selected by who is logged into ~/.claude, which is what lets
    /// the default and a profile hold one account without interfering.
    pub fn resolve_active(&self) -> Result<Target> {
        let Ok(key) = std::env::var("CSWAP_ACTIVE") else {
            return Ok(Target::Default);
        };
        if key.is_empty() || is_default_key(&key) {
            return Ok(Target::Default);
        }
        self.find(&key)
            .cloned()
            .map(Target::Profile)
            .with_context(|| {
                format!("CSWAP_ACTIVE={key} does not match any profile (see `cswap list`)")
            })
    }

    /// Resolve a user-typed key to a target, accepting `default`/`off`.
    pub fn resolve_key(&self, key: &str) -> Result<Target> {
        if is_default_key(key) {
            return Ok(Target::Default);
        }
        self.find(key)
            .cloned()
            .map(Target::Profile)
            .with_context(|| format!("no profile '{key}' (see `cswap list --quick`)"))
    }
}

/// One-time on-disk migration to the 0.6.1 layout.
///
/// Two shapes get folded in, both keyed by email in the end:
///   * 0.5 `[[account]]` — already email-keyed on disk, so only the TOML key
///     changes and the `accounts/` credential store moves aside.
///   * 0.6.0 `[[profile]]` with a `name` — the name becomes an alias and
///     `profiles/<name>` is renamed back to `profiles/<email>`.
///
/// Credentials are never copied INTO a profile here. An account that only ever
/// lived in the old store (it was the live login, so it never had a profile)
/// comes out with no credentials, and `cswap login <email>` gives it its own.
/// Seeding it from the store would hand two entities one refresh-token family.
pub fn migrate_on_disk() -> Result<()> {
    let path = paths::config_file();
    if !path.exists() {
        return Ok(());
    }
    let text = fs::read_to_string(&path)?;
    let raw: RawConfig = match toml::from_str(&text) {
        Ok(c) => c,
        Err(_) => return Ok(()), // load() will surface the real error later
    };
    let from_0_5 = !raw.accounts.is_empty();
    let named = raw
        .profiles
        .iter()
        .filter(|e| e.name.as_ref().is_some_and(|n| !n.is_empty()))
        .count();
    if !from_0_5 && named == 0 {
        return Ok(());
    }

    // 0.6.0 keyed directories by name; put them back under the email.
    let mut moved = 0;
    for entry in raw.profiles.iter() {
        let Some(name) = entry.name.as_ref().filter(|n| !n.is_empty()) else {
            continue;
        };
        let from = paths::profile_dir(name);
        let to = paths::profile_dir(&entry.email);
        if !from.is_dir() {
            continue;
        }
        // A 0.6.0 run may have left a compatibility symlink at the email path
        // pointing back at the name; it is in the way and it is ours to clear.
        if fs::symlink_metadata(&to).is_ok_and(|m| m.file_type().is_symlink()) {
            let points_here = fs::read_link(&to).map(|t| t == from).unwrap_or(false);
            if points_here {
                fs::remove_file(&to)?;
            }
        }
        if !to.exists() {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            if fs::rename(&from, &to).is_ok() {
                moved += 1;
            }
        }
    }

    // Keep the old store rather than deleting it: it holds real tokens, and a
    // user who wants them back can copy one in by hand.
    let store = paths::legacy_accounts_dir();
    let parked = store.with_file_name("accounts.pre-0.6.bak");
    let store_parked = store.is_dir() && !parked.exists() && fs::rename(&store, &parked).is_ok();

    let profiles: Vec<Profile> = raw
        .profiles
        .iter()
        .chain(raw.accounts.iter())
        .map(RawEntry::normalize)
        .collect();
    let cfg = Config { profiles };
    cfg.save()?;

    eprintln!(
        "cswap: migrated to the 0.6.1 model — one profile per account, keyed by \
         email, and `default` is the live ~/.claude (cswap only reads it)."
    );
    if moved > 0 {
        eprintln!(
            "cswap: moved {moved} profile director{} back under its email.",
            if moved == 1 { "y" } else { "ies" }
        );
    }
    let without_creds: Vec<&str> = cfg
        .profiles
        .iter()
        .filter(|p| {
            !paths::profile_dir(&p.email)
                .join(".credentials.json")
                .exists()
        })
        .map(|p| p.email.as_str())
        .collect();
    if !without_creds.is_empty() {
        eprintln!(
            "cswap: no login stored for {} — run: cswap login {}",
            without_creds.join(", "),
            without_creds[0]
        );
    }
    if store_parked {
        eprintln!(
            "cswap: the old credential store moved to {} (safe to delete).",
            parked.display()
        );
    }
    Ok(())
}

/// Reject a second profile for an account someone already holds. The default
/// is deliberately not consulted: it is allowed to share.
pub fn ensure_account_free(cfg: &Config, email: &str) -> Result<()> {
    if let Some(p) = cfg.profiles.iter().find(|p| p.email == email) {
        bail!(
            "{email} already has a profile (labelled '{}'). One profile per account — \
             use it, or `cswap remove {email}` first.",
            p.label()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prof(email: &str, aliases: &[&str]) -> Profile {
        Profile {
            email: email.into(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn load_str(text: &str) -> Config {
        let raw: RawConfig = toml::from_str(text).unwrap();
        Config {
            profiles: raw
                .profiles
                .iter()
                .chain(raw.accounts.iter())
                .map(RawEntry::normalize)
                .collect(),
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
        assert!(!valid_label("default"), "reserved: means ~/.claude");
        assert!(!valid_label("off"));
    }

    #[test]
    fn account_validation_guards_the_directory_name() {
        assert!(valid_account("you@corp.com"));
        assert!(!valid_account("nobody"), "must look like an address");
        assert!(!valid_account("a/b@x.com"), "would escape the profiles dir");
        assert!(!valid_account(".hidden@x.com"));
        assert!(!valid_account("has space@x.com"));
        assert!(!valid_account("default"));
    }

    #[test]
    fn find_by_email_and_alias() {
        let cfg = Config {
            profiles: vec![prof("a@x.com", &["alpha", "a1"]), prof("b@x.com", &[])],
        };
        assert_eq!(cfg.find("a@x.com").unwrap().email, "a@x.com");
        assert_eq!(cfg.find("alpha").unwrap().email, "a@x.com");
        assert_eq!(cfg.find("a1").unwrap().email, "a@x.com");
        assert!(cfg.find("zzz").is_none());
        assert_eq!(cfg.profiles[0].label(), "alpha");
        assert_eq!(cfg.profiles[1].label(), "b@x.com");
    }

    #[test]
    fn one_profile_per_account() {
        let cfg = Config {
            profiles: vec![prof("taken@x.com", &["work"])],
        };
        let err = ensure_account_free(&cfg, "taken@x.com")
            .unwrap_err()
            .to_string();
        assert!(err.contains("already has a profile"), "{err}");
        assert!(err.contains("work"), "names the existing label: {err}");
        assert!(ensure_account_free(&cfg, "free@x.com").is_ok());
    }

    #[test]
    fn default_key_resolves_to_the_default_target() {
        let cfg = Config::default();
        assert!(matches!(
            cfg.resolve_key("default").unwrap(),
            Target::Default
        ));
        assert!(matches!(cfg.resolve_key("off").unwrap(), Target::Default));
        assert!(cfg.resolve_key("nope").is_err());
    }

    #[test]
    fn reads_0_5_accounts_and_0_6_0_named_profiles_alike() {
        // 0.5: email-keyed accounts.
        let cfg = load_str("[[account]]\nemail = \"a@x.com\"\naliases = [\"one\"]\n");
        assert_eq!(cfg.profiles[0].email, "a@x.com");
        assert_eq!(cfg.profiles[0].aliases, vec!["one".to_string()]);

        // 0.6.0: a separate name key, which folds back to the front alias.
        let cfg =
            load_str("[[profile]]\nname = \"wadhwani\"\nemail = \"w@x.com\"\naliases = [\"w\"]\n");
        assert_eq!(cfg.profiles[0].email, "w@x.com");
        assert_eq!(
            cfg.profiles[0].aliases,
            vec!["wadhwani".to_string(), "w".to_string()],
            "the 0.6.0 name becomes the primary alias"
        );
        assert_eq!(cfg.profiles[0].label(), "wadhwani");
    }

    #[test]
    fn serializing_writes_profiles_keyed_by_email_with_no_name() {
        let cfg = Config {
            profiles: vec![prof("d@x.com", &["dev"])],
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(text.contains("[[profile]]"), "{text}");
        assert!(!text.contains("[[account]]"), "{text}");
        assert!(!text.contains("name ="), "the name key is gone: {text}");
        assert!(text.contains("email = \"d@x.com\""), "{text}");
        let back = load_str(&text);
        assert_eq!(back.profiles[0].aliases, vec!["dev".to_string()]);
    }
}
