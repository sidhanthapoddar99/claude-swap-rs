//! `cswap alias` — labels over email identities.
//!
//!   cswap alias list
//!   cswap alias create [ACCOUNT] [ALIAS]    (interactive when omitted)
//!   cswap alias remove [ALIAS]              (interactive when omitted)

use anyhow::{bail, Context, Result};

use crate::config::{valid_label, Config};
use crate::interactive;

pub fn list() -> Result<()> {
    let cfg = Config::load()?;
    if cfg.accounts.is_empty() {
        println!("No accounts yet — run `cswap login`.");
        return Ok(());
    }
    println!("{:<30} ALIASES", "EMAIL");
    for a in &cfg.accounts {
        let aliases = if a.aliases.is_empty() {
            "-".to_string()
        } else {
            a.aliases.join(", ")
        };
        println!("{:<30} {aliases}", a.email);
    }
    Ok(())
}

pub fn create(account: Option<String>, alias: Option<String>) -> Result<()> {
    let mut cfg = Config::load()?;
    let email = match account {
        Some(key) => cfg
            .find(&key)
            .with_context(|| format!("no account '{key}' (see `cswap list --quick`)"))?
            .email
            .clone(),
        None => interactive::pick_account(&cfg, "Add an alias for which account?", &[])?
            .expect("no extra items")
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
        bail!("'{alias}' is already used as an alias or email");
    }
    cfg.accounts
        .iter_mut()
        .find(|a| a.email == email)
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
        .accounts
        .iter()
        .flat_map(|a| a.aliases.iter().map(|al| (al.clone(), a.email.clone())))
        .collect();
    if all.is_empty() {
        bail!("there are no aliases to remove");
    }
    let target = match alias {
        Some(a) => a,
        None => {
            let items: Vec<String> = all
                .iter()
                .map(|(al, email)| format!("{al}  ({email})"))
                .collect();
            let idx = interactive::pick_string(&items, "Remove which alias?")?;
            all[idx].0.clone()
        }
    };
    let acct = cfg
        .accounts
        .iter_mut()
        .find(|a| a.aliases.contains(&target))
        .with_context(|| format!("no account has the alias '{target}'"))?;
    acct.aliases.retain(|al| *al != target);
    let email = acct.email.clone();
    cfg.save()?;
    println!("Removed alias '{target}' from {email}.");
    Ok(())
}
