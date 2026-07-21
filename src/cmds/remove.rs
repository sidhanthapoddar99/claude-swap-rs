//! `cswap remove <name>` — forget an account.
//!
//! Deletes the config entry, the stored credential backup, and the profile
//! directory. The profile contains symlinks into ~/.claude;
//! `fs::remove_dir_all` removes symlinks WITHOUT following them, so the
//! user's real Claude data is never touched.

use anyhow::{Context, Result};
use std::fs;

use crate::config::Config;
use crate::paths;

pub fn run(name: String) -> Result<()> {
    let mut cfg = Config::load()?;
    let acct = cfg
        .find(&name)
        .with_context(|| format!("no account '{name}' (see `cswap list`)"))?
        .clone();

    cfg.accounts.retain(|a| a.name != acct.name);
    if cfg.default.as_deref() == Some(acct.name.as_str()) {
        cfg.default = cfg.accounts.first().map(|a| a.name.clone());
    }
    cfg.save()?;

    for path in [
        paths::store_creds(&acct.name),
        paths::store_meta(&acct.name),
    ] {
        let _ = fs::remove_file(path);
    }
    let profile = paths::profile_dir(&acct.name);
    if profile.exists() {
        fs::remove_dir_all(&profile)
            .with_context(|| format!("failed to remove {}", profile.display()))?;
    }

    println!("Removed '{}' ({})", acct.name, acct.email);
    match &cfg.default {
        Some(d) => println!("default is now: {d}"),
        None => println!("no accounts left"),
    }
    Ok(())
}
