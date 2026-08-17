//! `cswap alias` — extra labels over profile names.
//!
//!   cswap alias list
//!   cswap alias create [PROFILE] [ALIAS]    (interactive when omitted)
//!   cswap alias remove [ALIAS]              (interactive when omitted)
//!
//! A profile IS its account, so the email is the key; an alias is a shorter
//! label for it. `default` has no aliases — it is a reserved word, not a
//! profile.

use anyhow::{bail, Context, Result};

use crate::config::{valid_label, Config};
use crate::interactive;

pub fn list() -> Result<()> {
    let cfg = Config::load()?;
    if cfg.profiles.is_empty() {
        println!("No profiles yet — run `cswap login <email>`.");
        return Ok(());
    }
    println!("{:<34} ALIASES", "ACCOUNT");
    for p in &cfg.profiles {
        let aliases = if p.aliases.is_empty() {
            "-".to_string()
        } else {
            p.aliases.join(", ")
        };
        println!("{:<34} {aliases}", p.email);
    }
    Ok(())
}

pub fn create(profile: Option<String>, alias: Option<String>) -> Result<()> {
    let mut cfg = Config::load()?;
    let email = match profile {
        Some(key) if crate::config::is_default_key(&key) => {
            bail!("`default` is the live ~/.claude, not a profile — it can't have aliases")
        }
        Some(key) => cfg
            .find(&key)
            .with_context(|| format!("no profile '{key}' (see `cswap list --quick`)"))?
            .email
            .clone(),
        None => interactive::pick_profile(&cfg, "Add an alias for which profile?")?
            .email
            .clone(),
    };
    let alias = match alias {
        Some(a) => a,
        None => interactive::input(&format!("New alias for {email}"))?,
    };
    if !valid_label(&alias) {
        bail!("invalid alias '{alias}' (use lowercase letters, digits, '-', '_', '.')");
    }
    if cfg.label_taken(&alias) {
        bail!("'{alias}' is already used as an alias or account");
    }
    cfg.profiles
        .iter_mut()
        .find(|p| p.email == email)
        .expect("resolved above")
        .aliases
        .push(alias.clone());
    cfg.save()?;
    println!("'{alias}' now points to {email}.");
    Ok(())
}

pub fn remove(alias: Option<String>) -> Result<()> {
    let mut cfg = Config::load()?;
    let all: Vec<(String, String)> = cfg
        .profiles
        .iter()
        .flat_map(|p| p.aliases.iter().map(|a| (a.clone(), p.email.clone())))
        .collect();
    if all.is_empty() {
        bail!("there are no aliases to remove");
    }
    let target = match alias {
        Some(a) => a,
        None => {
            let items: Vec<String> = all
                .iter()
                .map(|(a, email)| format!("{a}  ({email})"))
                .collect();
            let idx = interactive::pick_string(&items, "Remove which alias?")?;
            all[idx].0.clone()
        }
    };
    let prof = cfg
        .profiles
        .iter_mut()
        .find(|p| p.aliases.contains(&target))
        .with_context(|| format!("no profile has the alias '{target}'"))?;
    prof.aliases.retain(|a| *a != target);
    let email = prof.email.clone();
    cfg.save()?;
    println!("Removed alias '{target}' from {email}.");
    Ok(())
}
