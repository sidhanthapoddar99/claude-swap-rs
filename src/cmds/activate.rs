//! `cswap activate [NAME|default]` — per-terminal target selection.
//!
//! With no argument on a terminal, an interactive menu picks between `default`
//! and the profiles. The real work happens in the shell function installed by
//! `cswap shell-init`: it calls `--print` and evals the export line from
//! stdout; menus/feedback render on stderr.

use anyhow::Result;

use crate::config::{Config, Target};
use crate::{interactive, profile};

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
    // Pre-0.5.1 shell wrappers passed "${2:-}" — an EMPTY string when no
    // target was given — which made the picker branch unreachable. Treat
    // empty as absent so old snippets still sourced in open shells work.
    let target = match key.as_deref().filter(|k| !k.is_empty()) {
        Some(k) => cfg.resolve_key(k)?,
        None if interactive::on_tty() => {
            interactive::pick_target(&cfg, "Activate which profile (this shell)?")?
        }
        None => Target::Default, // non-interactive bare activate means "default"
    };
    match target {
        Target::Profile(p) => {
            // Export the EMAIL: it is the stable key, so the export keeps
            // working if the alias is renamed or removed later.
            println!("export CSWAP_ACTIVE='{}'", p.email);
            eprintln!(
                "cswap: active → {} ({}) [this shell only]",
                p.label(),
                p.email
            );
        }
        Target::Default => {
            println!("unset CSWAP_ACTIVE");
            let who = profile::live_email().unwrap_or_else(|| "nobody logged in".to_string());
            eprintln!("cswap: back to default ({who}) [live ~/.claude]");
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
