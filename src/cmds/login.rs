//! `cswap login` — register or refresh accounts. Accounts are keyed by
//! email; there is no separate name (aliases are the labels).
//!
//! Default: capture whoever is logged into the live ~/.claude.
//!   known email   -> relogin: refresh that account's stored tokens
//!   unknown email -> register under the email (+ optional alias)
//!
//! `--new`: log into a DIFFERENT account from inside cswap. Launches claude
//! in an empty staging profile (own CLAUDE_CONFIG_DIR): claude sees no
//! credentials and walks you through login; on exit cswap captures the
//! result and promotes the staging dir to the account's profile. The live
//! ~/.claude login is never touched.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::process::Command;

use crate::config::{valid_label, Account, Config};
use crate::{interactive, paths, profile};

pub fn run(alias_arg: Option<String>, new: bool) -> Result<()> {
    if new {
        return login_new(alias_arg);
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

    if cfg.find(&email).is_some() {
        // Relogin: refresh tokens; an --alias here simply adds the label.
        save_capture(&email, &creds_text, &oa)?;
        let profile_creds = paths::profile_dir(&email).join(".credentials.json");
        if profile_creds.exists() {
            profile::write_private(&profile_creds, &creds_text)?;
        }
        println!("Live ~/.claude login: {email}");
        println!("Refreshed stored credentials for this already-registered account.");
        if let Some(alias) = alias_arg {
            add_alias(&mut cfg, &email, &alias)?;
        } else {
            println!("(To add a different account: cswap login --new)");
        }
        return Ok(());
    }

    // New email -> register under it.
    save_capture(&email, &creds_text, &oa)?;
    register(&mut cfg, &email, alias_arg)?;
    Ok(())
}

/// `cswap login --new`: interactive login into a staging profile.
fn login_new(alias_arg: Option<String>) -> Result<()> {
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
        eprintln!("(claude exited with {status} — checking whether a login was captured anyway)");
    }

    let creds_text = fs::read_to_string(staging.join(".credentials.json")).map_err(|_| {
        let _ = fs::remove_dir_all(&staging);
        anyhow::anyhow!("no login captured — claude exited without completing a login")
    })?;
    let (email, oa) = identity_of(&creds_text, &staging_claude_json(&staging))?;

    let mut cfg = Config::load()?;
    if cfg.find(&email).is_some() {
        let _ = fs::remove_dir_all(&staging);
        bail!("you logged in as {email}, which is already registered (see `cswap list --quick`)");
    }

    save_capture(&email, &creds_text, &oa)?;

    // Promote staging to the real profile: credentials + .claude.json are
    // already in place; ensure() will add the share symlinks on first run.
    let profile_dir = paths::profile_dir(&email);
    if let Some(parent) = profile_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = fs::remove_dir_all(&profile_dir);
    fs::rename(&staging, &profile_dir).context("failed to promote staging profile")?;

    register(&mut cfg, &email, alias_arg)?;
    Ok(())
}

fn register(cfg: &mut Config, email: &str, alias_arg: Option<String>) -> Result<()> {
    let first = cfg.accounts.is_empty();
    cfg.accounts.push(Account::new(email.to_string()));
    if first {
        cfg.default = Some(email.to_string());
    }
    cfg.save()?;
    println!("Registered {email}");
    if first {
        println!("Set as default account.");
    }
    match alias_arg {
        Some(alias) => add_alias(cfg, email, &alias)?,
        // Offer an alias right away — select/input TUI, skippable with Enter.
        None if interactive::on_tty() => {
            let alias = interactive::input_optional(&format!("Alias for {email} (Enter to skip)"))?;
            if let Some(alias) = alias {
                add_alias(cfg, email, &alias)?;
            }
        }
        None => {}
    }
    Ok(())
}

fn add_alias(cfg: &mut Config, email: &str, alias: &str) -> Result<()> {
    if !valid_label(alias) {
        bail!("invalid alias '{alias}' (use lowercase letters, digits, '-', '_', '.')");
    }
    if cfg.label_taken(alias) {
        bail!("'{alias}' is already used as an alias or email");
    }
    cfg.accounts
        .iter_mut()
        .find(|a| a.email == email)
        .expect("caller resolved")
        .aliases
        .push(alias.to_string());
    cfg.save()?;
    println!("'{alias}' now points to {email}.");
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

fn save_capture(email: &str, creds_text: &str, oa: &Value) -> Result<()> {
    profile::write_private(&paths::store_creds(email), creds_text)?;
    let meta = json!({
        "oauthAccount": oa,
        "theme": live_claude_json().get("theme").and_then(Value::as_str).unwrap_or("dark"),
    });
    profile::write_private(
        &paths::store_meta(email),
        &serde_json::to_string_pretty(&meta)?,
    )
}
