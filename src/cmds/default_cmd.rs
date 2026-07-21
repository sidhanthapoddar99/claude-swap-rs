//! `cswap default [ALIAS|EMAIL]` — show or set the default account (what a
//! bare `claude` uses when nothing is activated). Stored as the email.

use anyhow::{Context, Result};

use crate::config::Config;
use crate::interactive;

pub fn run(key: Option<String>) -> Result<()> {
    let mut cfg = Config::load()?;
    let target = match key {
        Some(k) => cfg
            .find(&k)
            .with_context(|| format!("no account '{k}' (see `cswap list --quick`)"))?
            .clone(),
        None => {
            if interactive::on_tty() && !cfg.accounts.is_empty() {
                match interactive::pick_account(
                    &cfg,
                    "Set which account as default?",
                    &["(just show current)"],
                )? {
                    Some(a) => a.clone(),
                    None => {
                        show(&cfg);
                        return Ok(());
                    }
                }
            } else {
                show(&cfg);
                return Ok(());
            }
        }
    };
    let email = target.email.clone();
    cfg.default = Some(email.clone());
    cfg.save()?;
    println!("default → {email}");
    Ok(())
}

fn show(cfg: &Config) {
    match &cfg.default {
        Some(d) => println!("default: {d}"),
        None => println!("no default set — `cswap default <alias|email>`"),
    }
}
