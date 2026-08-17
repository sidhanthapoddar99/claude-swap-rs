//! Interactive pickers — used whenever a command that needs a target (or
//! alias) is invoked without one on a real terminal. All prompts render on
//! stderr so stdout stays clean for eval'd output (activate --print).

use anyhow::{bail, Context, Result};
use dialoguer::console::Term;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};

use crate::config::{Config, Profile, Target};
use crate::profile;

pub fn on_tty() -> bool {
    Term::stderr().is_term()
}

fn describe(p: &Profile) -> String {
    if p.aliases.is_empty() {
        p.email.clone()
    } else {
        format!("{}  ({})", p.label(), p.email)
    }
}

/// Arrow-key menu over `default` and every profile.
pub fn pick_target(cfg: &Config, prompt: &str) -> Result<Target> {
    if !on_tty() {
        bail!("not a terminal — pass a profile or `default` (see `cswap list --quick`)");
    }
    let who = profile::live_email().unwrap_or_else(|| "nobody logged in".to_string());
    let mut items = vec![format!("default  ({who})  [live ~/.claude]")];
    items.extend(cfg.profiles.iter().map(describe));
    let idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .items(&items)
        .default(0)
        .interact_on(&Term::stderr())
        .context("selection cancelled")?;
    Ok(match idx {
        0 => Target::Default,
        n => Target::Profile(cfg.profiles[n - 1].clone()),
    })
}

/// Arrow-key menu over profiles only — for commands that cannot act on the
/// default, because cswap owns no state for it (remove, alias).
pub fn pick_profile<'a>(cfg: &'a Config, prompt: &str) -> Result<&'a Profile> {
    if cfg.profiles.is_empty() {
        bail!("no profiles yet — run `cswap login <email>` first");
    }
    if !on_tty() {
        bail!("not a terminal — pass a profile (see `cswap list --quick`)");
    }
    let items: Vec<String> = cfg.profiles.iter().map(describe).collect();
    let idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .items(&items)
        .default(0)
        .interact_on(&Term::stderr())
        .context("selection cancelled")?;
    Ok(&cfg.profiles[idx])
}

pub fn pick_string(items: &[String], prompt: &str) -> Result<usize> {
    if !on_tty() {
        bail!("not a terminal — pass the value as an argument");
    }
    Select::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .items(items)
        .default(0)
        .interact_on(&Term::stderr())
        .context("selection cancelled")
}

pub fn input(prompt: &str) -> Result<String> {
    if !on_tty() {
        bail!("not a terminal — pass the value as an argument");
    }
    Input::<String>::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .interact_text_on(&Term::stderr())
        .context("input cancelled")
}

/// Optional text input: empty submission means "skip" (returns None).
pub fn input_optional(prompt: &str) -> Result<Option<String>> {
    if !on_tty() {
        return Ok(None);
    }
    let text = Input::<String>::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .allow_empty(true)
        .interact_text_on(&Term::stderr())
        .context("input cancelled")?;
    let text = text.trim().to_string();
    Ok(if text.is_empty() { None } else { Some(text) })
}

pub fn confirm(prompt: &str) -> Result<bool> {
    if !on_tty() {
        bail!("not a terminal — pass --yes to confirm non-interactively");
    }
    Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .default(false)
        .interact_on(&Term::stderr())
        .context("confirmation cancelled")
}
