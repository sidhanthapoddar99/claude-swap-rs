//! `cswap login` — register or refresh accounts.
//!
//! Default: capture whoever is logged into the live ~/.claude.
//!   known email   -> relogin: refresh that account's stored tokens
//!   unknown email -> register: name it, add to config
//! `--name` on an already-registered live login is an ERROR, not a silent
//! relogin — the name you typed must never be quietly discarded.
//!
//! `--new`: log into a DIFFERENT account from inside cswap. Launches claude
//! in an empty staging profile (own CLAUDE_CONFIG_DIR): claude sees no
//! credentials and walks you through login; on exit cswap captures the
//! result and promotes the staging dir to the account's profile. The live
//! ~/.claude login is never touched.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::process::Command;

use crate::config::{valid_name, Account, Config};
use crate::{paths, profile};

pub fn run(name_arg: Option<String>, new: bool) -> Result<()> {
    if new {
        return login_new(name_arg);
    }

    let creds_path = paths::live_credentials();
    let creds_text = fs::read_to_string(&creds_path).with_context(|| {
        format!(
            "no live Claude Code login found ({}) — run `claude` and log in first, \
             or use `cswap login --new` to log into a fresh account",
            creds_path.display()
        )
    })?;
    let (email, oa) = identity_of(&creds_text, &live_claude_json())?;

    let mut cfg = Config::load()?;

    if let Some(acct) = cfg.find(&email).cloned() {
        // The live login is already registered. A --name here is a mistake we
        // must not swallow: the user thinks they're adding a new account.
        if let Some(requested) = &name_arg {
            if *requested != acct.name {
                bail!(
                    "the live ~/.claude login is {email}, which is already registered as \
                     '{}' — `--name {requested}` would not add a new account.\n\
                     To register a different account, log into it first:\n  \
                     cswap login --new --name {requested}   (log in inside cswap; \
                     live login untouched)\n\
                     or run `claude /login`, switch accounts, then `cswap login --name {requested}`.",
                    acct.name
                );
            }
        }
        save_capture(&acct.name, &creds_text, &oa)?;
        let profile_creds = paths::profile_dir(&acct.name).join(".credentials.json");
        if profile_creds.exists() {
            profile::write_private(&profile_creds, &creds_text)?;
        }
        println!("Live ~/.claude login: {email}");
        println!(
            "Refreshed stored credentials for existing account '{}'.",
            acct.name
        );
        println!("(To add a different account: cswap login --new)");
        return Ok(());
    }

    // New email -> register.
    let name = resolve_name(name_arg, &email, &cfg)?;
    save_capture(&name, &creds_text, &oa)?;
    register(&mut cfg, &name, &email)?;
    Ok(())
}

/// `cswap login --new`: interactive login into a staging profile.
fn login_new(name_arg: Option<String>) -> Result<()> {
    let staging = paths::data_dir().join("staging-login");
    let _ = fs::remove_dir_all(&staging); // leftovers from an aborted attempt
    fs::create_dir_all(&staging)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))?;
    }
    // Seed enough to skip theme onboarding; there are no credentials, so
    // claude goes straight into its login flow.
    let theme = live_claude_json()
        .get("theme")
        .and_then(Value::as_str)
        .unwrap_or("dark")
        .to_string();
    profile::write_private(
        &staging.join(".claude.json"),
        &serde_json::to_string_pretty(&json!({
            "hasCompletedOnboarding": true,
            "theme": theme,
        }))?,
    )?;

    println!("Launching claude in a clean profile — log in with the NEW account,");
    println!("then exit claude (/exit) to finish registration. Your current");
    println!("~/.claude login is not touched.");
    println!();

    let claude = crate::cmds::run::find_claude()?;
    let mut cmd = Command::new(&claude);
    cmd.env("CLAUDE_CONFIG_DIR", &staging);
    for var in crate::cmds::run::SCRUBBED {
        cmd.env_remove(var);
    }
    let status = cmd.status().context("failed to launch claude")?;
    if !status.success() {
        // claude exits 0 on normal /exit; nonzero usually means aborted.
        eprintln!("(claude exited with {status} — checking whether a login was captured anyway)");
    }

    let creds_text = fs::read_to_string(staging.join(".credentials.json")).map_err(|_| {
        let _ = fs::remove_dir_all(&staging);
        anyhow::anyhow!("no login captured — claude exited without completing a login")
    })?;
    let (email, oa) = identity_of(&creds_text, &staging_claude_json(&staging))?;

    let mut cfg = Config::load()?;
    if let Some(existing) = cfg.find(&email) {
        let name = existing.name.clone();
        let _ = fs::remove_dir_all(&staging);
        bail!("you logged in as {email}, which is already registered as '{name}'");
    }

    let name = resolve_name(name_arg, &email, &cfg).inspect_err(|_| {
        let _ = fs::remove_dir_all(&staging);
    })?;
    save_capture(&name, &creds_text, &oa)?;

    // Promote staging to the real profile: credentials + .claude.json are
    // already in place; ensure() will add the share symlinks on first run.
    let profile_dir = paths::profile_dir(&name);
    if let Some(parent) = profile_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = fs::remove_dir_all(&profile_dir);
    fs::rename(&staging, &profile_dir).context("failed to promote staging profile")?;

    register(&mut cfg, &name, &email)?;
    Ok(())
}

/// Extract (email, oauthAccount) for a captured credential set.
fn identity_of(creds_text: &str, claude_json: &Value) -> Result<(String, Value)> {
    let creds: Value =
        serde_json::from_str(creds_text).context(".credentials.json is not valid JSON")?;
    let oauth = creds
        .get("claudeAiOauth")
        .and_then(Value::as_object)
        .context("credentials carry no claudeAiOauth — the login did not complete")?;
    if oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .unwrap_or("")
        .is_empty()
    {
        bail!("credentials have no refreshToken — re-login with `claude /login` first");
    }
    let oa = claude_json
        .get("oauthAccount")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let email = oa
        .get("emailAddress")
        .and_then(Value::as_str)
        .context(
            "no oauthAccount.emailAddress recorded — run claude once so it stores who is logged in",
        )?
        .to_string();
    Ok((email, oa))
}

fn live_claude_json() -> Value {
    fs::read_to_string(paths::claude_json())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

fn staging_claude_json(staging: &std::path::Path) -> Value {
    fs::read_to_string(staging.join(".claude.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

fn resolve_name(name_arg: Option<String>, email: &str, cfg: &Config) -> Result<String> {
    let name = match name_arg {
        Some(n) => n,
        None => prompt_name(email)?,
    };
    if !valid_name(&name) {
        bail!("invalid name '{name}' (use lowercase letters, digits, '-', '_', '.')");
    }
    if cfg.label_taken(&name) {
        bail!("'{name}' is already used as an account name or alias (see `cswap list`)");
    }
    Ok(name)
}

fn register(cfg: &mut Config, name: &str, email: &str) -> Result<()> {
    let first = cfg.accounts.is_empty();
    cfg.accounts.push(Account {
        name: name.to_string(),
        email: email.to_string(),
        aliases: Vec::new(),
        isolated: false,
    });
    if first {
        cfg.default = Some(name.to_string());
    }
    cfg.save()?;
    println!("Registered '{name}' ({email})");
    if first {
        println!("Set as default account.");
    }
    Ok(())
}

fn save_capture(name: &str, creds_text: &str, oa: &Value) -> Result<()> {
    profile::write_private(&paths::store_creds(name), creds_text)?;
    let meta = json!({
        "oauthAccount": oa,
        "theme": live_claude_json().get("theme").and_then(Value::as_str).unwrap_or("dark"),
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
