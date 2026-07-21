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
        // Reaching the binary directly means the cswap() shell function did
        // not intercept — the integration isn't loaded in THIS shell. A picker
        // here would be a lie: a child process can't set the parent's env.
        eprintln!("cswap: activate needs the shell integration to take effect.");
        if rc_has_integration() {
            eprintln!("It's already in your shell rc — this terminal just predates it.");
            eprintln!("Open a new terminal, or run: source ~/.zshrc   (or ~/.bashrc)");
        } else {
            eprintln!("Add this to your ~/.zshrc or ~/.bashrc, then open a new terminal:");
            eprintln!("  eval \"$(cswap shell-init zsh)\"   # or: bash");
        }
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

/// Is the shell-init eval block already present in ~/.zshrc or ~/.bashrc?
/// (The installer writes it inside `# >>> cswap shell integration >>>`
/// markers, but any hand-added `cswap shell-init` eval counts too.)
fn rc_has_integration() -> bool {
    let home = crate::paths::home();
    [".zshrc", ".bashrc"].iter().any(|rc| {
        std::fs::read_to_string(home.join(rc))
            .map(|text| text.contains("cswap shell-init"))
            .unwrap_or(false)
    })
}
