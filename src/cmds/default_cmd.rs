//! `cswap default` — report who the live ~/.claude is. Read-only.
//!
//! `default` is its own entity: the ~/.claude that does not know cswap exists.
//! It is not a pointer at a profile and no profile is a pointer at it. cswap
//! reads it and nothing more, so there is no swap to perform here — changing
//! who ~/.claude is means logging in with claude itself.

use anyhow::{bail, Result};

use crate::config::Config;
use crate::profile;

pub fn run(key: Option<String>) -> Result<()> {
    if let Some(key) = key {
        bail!(
            "`cswap default {key}` no longer swaps the live login.\n\
             The default IS ~/.claude — a separate entity that cswap only reads.\n\
             To use {key} instead: `cswap activate {key}` (this terminal) or \
             `cswap run {key}` (once).\n\
             To change ~/.claude itself, log in with claude directly: \
             `command claude` then /login."
        );
    }
    let cfg = Config::load()?;
    match profile::live_email() {
        Some(email) => {
            println!("default: {email}  (the live ~/.claude — cswap only reads it)");
            if cfg.profiles.iter().any(|p| p.email == email) {
                println!(
                    "a profile holds this account too, independently — that is the \n\
                     one overlap cswap allows, because `default` is not a profile."
                );
            }
        }
        None => println!("default: (nobody logged into ~/.claude — run `claude` and log in)"),
    }
    Ok(())
}
