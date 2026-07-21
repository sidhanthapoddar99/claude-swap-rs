//! `cswap alias` — additional labels for accounts.
//!
//!   cswap alias                 list all aliases
//!   cswap alias work w          add alias 'w' for account 'work'
//!   cswap alias --remove w      remove alias 'w'
//!
//! Aliases resolve everywhere a name does: activate, run, default, remove.
//! Email stays the unique identity; name + aliases are labels over it.

use anyhow::{bail, Context, Result};

use crate::config::{valid_name, Config};

pub fn run(account: Option<String>, alias: Option<String>, remove: bool) -> Result<()> {
    let mut cfg = Config::load()?;

    if remove {
        // `cswap alias --remove <alias>`: the single positional is the alias.
        let target = account
            .or(alias)
            .context("usage: cswap alias --remove <alias>")?;
        let acct = cfg
            .accounts
            .iter_mut()
            .find(|a| a.aliases.iter().any(|al| *al == target))
            .with_context(|| format!("no account has the alias '{target}'"))?;
        acct.aliases.retain(|al| *al != target);
        let name = acct.name.clone();
        cfg.save()?;
        println!("Removed alias '{target}' from '{name}'.");
        return Ok(());
    }

    match (account, alias) {
        (Some(account), Some(alias)) => {
            if !valid_name(&alias) {
                bail!("invalid alias '{alias}' (use lowercase letters, digits, '-', '_', '.')");
            }
            if cfg.label_taken(&alias) {
                bail!("'{alias}' is already used as an account name or alias");
            }
            let acct = cfg
                .find(&account)
                .with_context(|| format!("no account '{account}' (see `cswap list`)"))?;
            let name = acct.name.clone();
            cfg.accounts
                .iter_mut()
                .find(|a| a.name == name)
                .expect("just found")
                .aliases
                .push(alias.clone());
            cfg.save()?;
            println!("'{alias}' now points to '{name}'.");
            Ok(())
        }
        (None, None) => {
            if cfg.accounts.is_empty() {
                println!("No accounts yet.");
                return Ok(());
            }
            for a in &cfg.accounts {
                let aliases = if a.aliases.is_empty() {
                    "-".to_string()
                } else {
                    a.aliases.join(", ")
                };
                println!("{:<14} {:<30} {aliases}", a.name, a.email);
            }
            Ok(())
        }
        _ => bail!("usage: cswap alias <account> <alias>  |  cswap alias --remove <alias>"),
    }
}
