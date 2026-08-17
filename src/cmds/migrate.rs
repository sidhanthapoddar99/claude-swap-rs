//! `cswap migrate` — rewrite a pre-0.6.1 config, on purpose.
//!
//! Migration used to run from `main()` before clap had parsed anything, so
//! `cswap --version` rewrote config.toml and renamed profile directories. It
//! did that to a live session once and the session's transcripts went to a
//! path nothing else could read. A migration is a mutation; a mutation gets a
//! command of its own, a confirmation, and a check for running sessions.

use anyhow::{bail, Result};

use crate::{config, interactive};

pub fn run(yes: bool) -> Result<()> {
    if !config::needs_migration() {
        println!("Config is already in the current format. Nothing to do.");
        return Ok(());
    }

    println!("This rewrites ~/.config/cswap/config.toml:");
    println!("  · `[[account]]` entries become `[[profile]]` (same email key, no directory moves)");
    println!("  · a 0.6.0 `name` folds into the alias list, and profiles/<name> moves to profiles/<email>");
    println!("  · the old accounts/ credential store moves to accounts.pre-0.6.bak, nothing copied out of it");
    println!();

    if !yes && interactive::on_tty() && !interactive::confirm("Migrate now?")? {
        bail!("aborted — config left as it is");
    }

    if config::migrate_on_disk()? {
        println!("Done. Check it with: cswap list --quick");
    }
    Ok(())
}
