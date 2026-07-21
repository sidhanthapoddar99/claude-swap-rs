//! `cswap remove [ALIAS|EMAIL]` — forget a registered profile.
//!
//! Interactive account picker when no argument is given; always ends with a
//! confirmation (or `--yes` for scripts). Deletes the config entry, stored
//! tokens, and the profile dir. The profile contains symlinks into ~/.claude;
//! `remove_dir_all` removes symlinks WITHOUT following them, so the user's
//! real Claude data is never touched.

use anyhow::{bail, Context, Result};
use std::fs;

use crate::config::Config;
use crate::interactive;
use crate::paths;

pub fn run(key: Option<String>, yes: bool) -> Result<()> {
    let mut cfg = Config::load()?;
    let acct = match key {
        Some(k) => cfg
            .find(&k)
            .with_context(|| format!("no account '{k}' (see `cswap list --quick`)"))?
            .clone(),
        None => interactive::pick_account(&cfg, "Remove which account?", &[])?
            .expect("no extra items")
            .clone(),
    };

    if !yes {
        let ok = interactive::confirm(&format!(
            "Remove {} ({})? Stored tokens and the profile dir will be deleted \
             (your ~/.claude data is untouched)",
            acct.label(),
            acct.email
        ))?;
        if !ok {
            bail!("aborted — nothing removed");
        }
    }

    cfg.accounts.retain(|a| a.email != acct.email);
    if cfg.default.as_deref() == Some(acct.email.as_str()) {
        cfg.default = cfg.accounts.first().map(|a| a.email.clone());
    }
    cfg.save()?;

    for path in [
        paths::store_creds(&acct.email),
        paths::store_meta(&acct.email),
    ] {
        let _ = fs::remove_file(path);
    }
    let profile = paths::profile_dir(&acct.email);
    if profile.exists() {
        fs::remove_dir_all(&profile)
            .with_context(|| format!("failed to remove {}", profile.display()))?;
    }

    println!("Removed {} ({})", acct.label(), acct.email);
    match &cfg.default {
        Some(d) => println!("default is now: {d}"),
        None => println!("no accounts left"),
    }
    Ok(())
}
