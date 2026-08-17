//! `cswap remove [NAME]` — forget a profile.
//!
//! Interactive profile picker when no argument is given; always ends with a
//! confirmation (or `--yes` for scripts). Deletes the config entry and the
//! profile directory, which holds that profile's only credentials. The
//! directory is mostly symlinks into ~/.claude; `remove_dir_all` removes
//! symlinks WITHOUT following them, so the user's real Claude data is safe.
//!
//! `default` cannot be removed: cswap owns no state for it.

use anyhow::{bail, Context, Result};

use crate::config::Config;
use crate::{interactive, profile};

pub fn run(key: Option<String>, yes: bool) -> Result<()> {
    let mut cfg = Config::load()?;
    let prof = match key {
        Some(k) if crate::config::is_default_key(&k) => bail!(
            "`default` is the live ~/.claude, not a cswap profile — there is nothing to remove.\n\
             To log out of it, use claude itself: `command claude` then /logout."
        ),
        Some(k) => cfg
            .find(&k)
            .with_context(|| format!("no profile '{k}' (see `cswap list --quick`)"))?
            .clone(),
        None => interactive::pick_profile(&cfg, "Remove which profile?")?.clone(),
    };

    // Deleting the directory under a running claude leaves that process writing
    // to paths nothing else can resolve, and it loses the session when it exits.
    let sessions = profile::live_sessions(&crate::paths::profile_dir(&prof.email));
    if !sessions.is_empty() {
        bail!(
            "claude is running as {} (pid {}) — exit it first.\n\
             Removing the directory underneath a live session strands whatever it \
             is still writing.",
            prof.email,
            sessions
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let has_creds = profile::has_login(&prof.email);
    if !yes {
        let warning = if has_creds {
            " Its login is stored only here, so you will have to log in again"
        } else {
            ""
        };
        let ok = interactive::confirm(&format!(
            "Remove the profile for {}?{warning}. Your ~/.claude data is untouched",
            prof.email
        ))?;
        if !ok {
            bail!("aborted — nothing removed");
        }
    }

    cfg.profiles.retain(|p| p.email != prof.email);
    cfg.save()?;
    crate::cmds::login::remove_dir(&prof.email)?;

    println!("Removed the profile for {}", prof.email);
    // The default is a separate entity, so removing a profile never changes it
    // — not even when both held the same account.
    if profile::live_email().as_deref() == Some(prof.email.as_str()) {
        println!(
            "note: {} is still the default (~/.claude). That login was always separate \
             and is unaffected.",
            prof.email
        );
    }
    Ok(())
}
