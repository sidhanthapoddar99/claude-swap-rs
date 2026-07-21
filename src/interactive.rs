//! Interactive pickers — used whenever a command that needs an account (or
//! alias) is invoked without one on a real terminal. All prompts render on
//! stderr so stdout stays clean for eval'd output (activate --print).

use anyhow::{bail, Context, Result};
use dialoguer::console::Term;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};

use crate::config::{Account, Config};

pub fn on_tty() -> bool {
    Term::stderr().is_term()
}

/// Arrow-key menu over all accounts. `extra` adds trailing non-account
/// choices (e.g. "(back to default)"); returns None when one of those wins.
pub fn pick_account<'a>(
    cfg: &'a Config,
    prompt: &str,
    extra: &[&str],
) -> Result<Option<&'a Account>> {
    if cfg.accounts.is_empty() {
        bail!("no accounts yet — run `cswap login` first");
    }
    if !on_tty() {
        bail!("not a terminal — pass an alias or email (see `cswap list --quick`)");
    }
    let mut items: Vec<String> = cfg
        .accounts
        .iter()
        .map(|a| {
            if a.aliases.is_empty() {
                a.email.clone()
            } else {
                format!("{}  ({})", a.label(), a.email)
            }
        })
        .collect();
    items.extend(extra.iter().map(|s| s.to_string()));
    let idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .items(&items)
        .default(0)
        .interact_on(&Term::stderr())
        .context("selection cancelled")?;
    Ok(cfg.accounts.get(idx))
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
