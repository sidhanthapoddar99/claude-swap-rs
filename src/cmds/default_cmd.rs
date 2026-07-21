//! `cswap default [NAME]` — show or set the default account (used when no
//! terminal has activated anything). Accepts name or email.

use anyhow::{Context, Result};

use crate::config::Config;

pub fn run(name: Option<String>) -> Result<()> {
    let mut cfg = Config::load()?;
    match name {
        None => {
            match &cfg.default {
                Some(d) => println!("default: {d}"),
                None => println!("no default set — `cswap default <name>`"),
            }
            Ok(())
        }
        Some(key) => {
            let acct = cfg
                .find(&key)
                .with_context(|| format!("no account '{key}' (see `cswap list`)"))?;
            let name = acct.name.clone();
            let email = acct.email.clone();
            cfg.default = Some(name.clone());
            cfg.save()?;
            println!("default → {name} ({email})");
            Ok(())
        }
    }
}
