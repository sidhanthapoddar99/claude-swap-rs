//! `cswap default [ALIAS|EMAIL]` — show or swap the default account.
//!
//! There is no stored default: the default IS the live ~/.claude login. So
//! `cswap default` (no arg) reports who is live and whether they're a
//! registered account, and `cswap default <x>` SWAPS the live login by
//! copying x's credentials into ~/.claude (the one command that writes there).
//!
//! Guard: if the account being displaced is not registered, its credentials
//! aren't stored anywhere and the swap would lose them for good — so we make
//! the user type `yes` (or pass --yes), pointing them at `cswap login` to keep
//! it instead.

use anyhow::{Context, Result};

use crate::config::Config;
use crate::{interactive, profile};

pub fn run(key: Option<String>, yes: bool) -> Result<()> {
    let cfg = Config::load()?;
    let target = match key {
        Some(k) => cfg
            .find(&k)
            .with_context(|| format!("no account '{k}' (see `cswap list --quick`)"))?
            .clone(),
        None => {
            // No arg on a terminal: offer the picker; otherwise just report.
            if interactive::on_tty() && !cfg.accounts.is_empty() {
                match interactive::pick_account(
                    &cfg,
                    "Swap the default (live ~/.claude) to which account?",
                    &["(just show current)"],
                )? {
                    Some(a) => a.clone(),
                    None => return show(&cfg),
                }
            } else {
                return show(&cfg);
            }
        }
    };

    let outgoing = profile::live_email();
    if outgoing.as_deref() == Some(target.email.as_str()) {
        println!("{} is already the default (live ~/.claude).", target.email);
        return Ok(());
    }

    // The outgoing live login is about to be overwritten.
    //   Registered   -> snapshot its CURRENT live tokens first: claude rotates
    //                   refresh tokens in ~/.claude while an account is live,
    //                   so the login-time store copy may already be stale.
    //   Unregistered -> its credentials are stored nowhere; require an explicit
    //                   typed `yes` (or --yes) before destroying them.
    if let Some(out) = &outgoing {
        if cfg.find(out).is_some() {
            profile::capture_live_into_store(out)
                .with_context(|| format!("failed to back up the outgoing login {out}"))?;
        } else {
            eprintln!(
                "The current live ~/.claude login is {out}, which is NOT registered.\n\
                 Swapping will overwrite it and its credentials are stored nowhere —\n\
                 this cannot be undone. To keep it, cancel and run `cswap login` first."
            );
            if !yes && !interactive::type_to_confirm("Type 'yes' to overwrite it", "yes")? {
                anyhow::bail!("cancelled — {out} left as the default");
            }
        }
    }

    profile::promote_to_live(&target)
        .with_context(|| format!("failed to swap the live login to {}", target.email))?;
    println!(
        "default → {} (swapped the live ~/.claude login)",
        target.email
    );
    if std::env::var_os("CSWAP_ACTIVE").is_some_and(|v| !v.is_empty()) {
        eprintln!("note: this shell still has an account activated — `cswap activate default` to follow the new default.");
    }
    Ok(())
}

/// Report the live identity and whether cswap knows it.
fn show(cfg: &Config) -> Result<()> {
    match profile::live_email() {
        Some(email) => {
            let status = if cfg.find(&email).is_some() {
                "registered"
            } else {
                "not registered"
            };
            println!("default: {email} ({status})");
        }
        None => println!("default: (nobody logged into ~/.claude — run `claude` and log in)"),
    }
    Ok(())
}
