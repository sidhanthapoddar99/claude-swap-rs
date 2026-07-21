//! `cswap activate [NAME]` — per-terminal account selection.
//!
//! A child process cannot set env vars in the parent shell, so the real work
//! happens in the shell function installed by `cswap shell-init`: it calls
//! `cswap activate --print <name>` and evals the export line we emit on
//! stdout (human feedback goes to stderr).

use anyhow::{Context, Result};

use crate::config::Config;

pub fn run(name: Option<String>, print: bool) -> Result<()> {
    let target = name.filter(|n| n != "default" && n != "off");
    if !print {
        // Called without the shell wrapper — can't affect the parent shell.
        eprintln!("cswap: activate needs the shell integration to take effect.");
        eprintln!("Add this to your ~/.zshrc or ~/.bashrc, then open a new terminal:");
        eprintln!("  eval \"$(cswap shell-init zsh)\"   # or: bash");
        return Ok(());
    }
    match target {
        Some(n) => {
            let cfg = Config::load()?;
            let acct = cfg
                .find(&n)
                .with_context(|| format!("no account '{n}' (see `cswap list`)"))?;
            println!("export CSWAP_ACTIVE='{}'", acct.name);
            eprintln!(
                "cswap: active → {} ({}) [this shell only]",
                acct.name, acct.email
            );
        }
        None => {
            println!("unset CSWAP_ACTIVE");
            eprintln!("cswap: back to the default account");
        }
    }
    Ok(())
}
