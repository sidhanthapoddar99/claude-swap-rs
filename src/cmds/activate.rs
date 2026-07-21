//! `cswap activate [ALIAS|EMAIL]` — per-terminal account selection.
//!
//! With no argument on a terminal, an interactive menu picks the account
//! (with a "back to default" choice). The real work happens in the shell
//! function installed by `cswap shell-init`: it calls `--print` and evals
//! the export line from stdout; menus/feedback render on stderr.

use anyhow::{Context, Result};

use crate::config::Config;
use crate::interactive;

pub fn run(key: Option<String>, print: bool) -> Result<()> {
    if !print {
        eprintln!("cswap: activate needs the shell integration to take effect.");
        eprintln!("Add this to your ~/.zshrc or ~/.bashrc, then open a new terminal:");
        eprintln!("  eval \"$(cswap shell-init zsh)\"   # or: bash");
        return Ok(());
    }
    let cfg = Config::load()?;
    let target = match key.as_deref() {
        Some("default") | Some("off") => None,
        Some(k) => Some(
            cfg.find(k)
                .with_context(|| format!("no account '{k}' (see `cswap list --quick`)"))?,
        ),
        None if interactive::on_tty() => interactive::pick_account(
            &cfg,
            "Activate which account (this shell)?",
            &["(back to default)"],
        )?,
        None => None, // non-interactive bare activate keeps meaning "default"
    };
    match target {
        Some(a) => {
            println!("export CSWAP_ACTIVE='{}'", a.email);
            eprintln!(
                "cswap: active → {} ({}) [this shell only]",
                a.label(),
                a.email
            );
        }
        None => {
            println!("unset CSWAP_ACTIVE");
            eprintln!("cswap: back to the default account");
        }
    }
    Ok(())
}
