//! `cswap login` — capture the account currently logged into ~/.claude.
//!
//! Known email  -> relogin: refresh the stored tokens for that account.
//! Unknown email -> register: prompt for a name, add to config.
//! Reads ~/.claude/.credentials.json and ~/.claude.json; writes only cswap's own store.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::io::{self, IsTerminal, Write};

use crate::config::{valid_name, Account, Config};
use crate::{paths, profile};

pub fn run(name_arg: Option<String>) -> Result<()> {
    let creds_path = paths::live_credentials();
    let creds_text = fs::read_to_string(&creds_path).with_context(|| {
        format!(
            "no live Claude Code login found ({}) — run `claude` and log in first",
            creds_path.display()
        )
    })?;
    let creds: Value =
        serde_json::from_str(&creds_text).context("live .credentials.json is not valid JSON")?;
    let oauth = creds
        .get("claudeAiOauth")
        .and_then(Value::as_object)
        .context("live credentials carry no claudeAiOauth — log in with `claude` first")?;
    if oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .unwrap_or("")
        .is_empty()
    {
        bail!("live credentials have no refreshToken; re-login with `claude /login` first");
    }

    let live_cfg: Value = fs::read_to_string(paths::claude_json())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    let oa = live_cfg
        .get("oauthAccount")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let email = oa
        .get("emailAddress")
        .and_then(Value::as_str)
        .context("~/.claude.json has no oauthAccount.emailAddress — run `claude` once so it records who is logged in")?
        .to_string();

    let mut cfg = Config::load()?;

    if let Some(acct) = cfg.find(&email).cloned() {
        // Relogin: refresh stored tokens + meta for the existing account.
        save_capture(&acct.name, &creds_text, &oa, &live_cfg)?;
        // The profile copy (if any) is the live store — update it too so the
        // fresh login wins over whatever the profile last held.
        let profile_creds = paths::profile_dir(&acct.name).join(".credentials.json");
        if profile_creds.exists() {
            profile::write_private(&profile_creds, &creds_text)?;
        }
        println!("Updated credentials for '{}' ({email})", acct.name);
        return Ok(());
    }

    // Register: new account.
    let name = match name_arg {
        Some(n) => n,
        None => prompt_name(&email)?,
    };
    if !valid_name(&name) {
        bail!("invalid name '{name}' (use lowercase letters, digits, '-', '_', '.')");
    }
    if cfg.find(&name).is_some() {
        bail!("an account named '{name}' already exists (see `cswap list`)");
    }

    save_capture(&name, &creds_text, &oa, &live_cfg)?;
    let first = cfg.accounts.is_empty();
    cfg.accounts.push(Account {
        name: name.clone(),
        email: email.clone(),
        isolated: false,
    });
    if first {
        cfg.default = Some(name.clone());
    }
    cfg.save()?;

    println!("Registered '{name}' ({email})");
    if first {
        println!("Set as default account.");
    }
    Ok(())
}

fn save_capture(name: &str, creds_text: &str, oa: &Value, live_cfg: &Value) -> Result<()> {
    profile::write_private(&paths::store_creds(name), creds_text)?;
    let meta = json!({
        "oauthAccount": oa,
        "theme": live_cfg.get("theme").and_then(Value::as_str).unwrap_or("dark"),
    });
    profile::write_private(
        &paths::store_meta(name),
        &serde_json::to_string_pretty(&meta)?,
    )
}

fn prompt_name(email: &str) -> Result<String> {
    if !io::stdin().is_terminal() {
        bail!("not a terminal — pass the account name: cswap login --name <name>");
    }
    print!("Name for {email} (e.g. personal, dev, work): ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}
