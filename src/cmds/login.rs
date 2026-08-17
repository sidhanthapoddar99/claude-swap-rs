//! `cswap login <EMAIL>` — create a profile by logging into it.
//!
//! One flow, always the same: build `profiles/<email>`, link it to ~/.claude,
//! launch claude inside it with no credentials so claude walks its own login,
//! then check that the account which arrived is the one you asked for. The live
//! ~/.claude login is never read, written or copied.
//!
//! Why you name the account up front: the directory is keyed by it, and it must
//! exist and be linked BEFORE claude runs. Staging under a temporary name and
//! renaming afterwards is what the old `login --new` did, and it cost twice —
//! claude created real directories that shadowed the share links forever, and
//! the paths it recorded into ~/.claude still point at the staging directory.
//! A directory that never moves has neither problem.
//!
//! Why there is no "adopt the current login" mode: copying credentials out of
//! ~/.claude would hand the profile and the default ONE refresh-token family.
//! Claude rotates refresh tokens in place, so whichever side ran last leaves
//! the other holding a dead ancestor. Every entity does its own login.

use anyhow::{bail, Context, Result};
use std::fs;
use std::process::Command;

use crate::config::{ensure_account_free, valid_account, valid_label, Config, Profile};
use crate::{interactive, paths, profile};

pub fn run(key: Option<String>, alias_arg: Option<String>, yes: bool) -> Result<()> {
    let mut cfg = Config::load()?;

    let key = match key {
        Some(k) => k,
        None if interactive::on_tty() => {
            interactive::input("Which account? (the email you'll log in as)")?
        }
        None => bail!("which account? pass the email: cswap login <email>"),
    };
    let key = key.trim().to_string();

    // An existing profile can be named by email OR by one of its aliases; a new
    // one has to be named by the email, since that is what keys the directory.
    let existing = cfg.find(&key).cloned();
    let email = match &existing {
        Some(p) => p.email.clone(),
        None => {
            if !valid_account(&key) {
                bail!(
                    "'{key}' is not an account email and no profile matches it.\n\
                     To create one: cswap login you@example.com"
                );
            }
            key.clone()
        }
    };

    let prof = existing
        .clone()
        .unwrap_or_else(|| Profile::new(email.clone()));
    let dir = profile::scaffold(&prof)?;
    let creds = profile::creds_path(&email);
    let fresh_profile = existing.is_none();

    // A relogin has to start from no credentials, or claude sees a valid
    // session and never offers the login. That discards this profile's tokens,
    // so ask first.
    if creds.exists() {
        if !yes
            && !interactive::confirm(&format!(
                "Profile {email} is already logged in. Log in again? Its current tokens \
                 are discarded (nothing outside this profile is touched)"
            ))?
        {
            bail!("aborted — {email} left as it is");
        }
        fs::remove_file(&creds).with_context(|| format!("failed to clear {}", creds.display()))?;
    }

    println!("Launching claude in the profile for {email} — log in as that account.");
    println!("Exit claude (/exit) when the login is done. Your ~/.claude login is not touched.");
    println!();

    let claude = crate::cmds::run::find_claude()?;
    let mut cmd = Command::new(&claude);
    cmd.env("CLAUDE_CONFIG_DIR", &dir);
    for var in crate::cmds::run::SCRUBBED {
        cmd.env_remove(var);
    }
    let status = cmd.status().context("failed to launch claude")?;
    if !status.success() {
        eprintln!("(claude exited with {status} — checking whether a login was captured anyway)");
    }

    let arrived = match verify_login(&creds, &email) {
        Ok(a) => a,
        Err(e) => {
            // Nothing usable landed: don't leave a half-built profile behind.
            if fresh_profile {
                let _ = remove_dir(&email);
            }
            return Err(e);
        }
    };

    // The directory is keyed by the account, so a mismatch cannot just be
    // accepted — it would file one account's tokens under another's name.
    if arrived != email {
        let _ = fs::remove_file(&creds);
        if fresh_profile {
            let _ = remove_dir(&email);
        }
        bail!(
            "you logged in as {arrived}, but this profile is for {email}.\n\
             Nothing was saved. To register that account instead: cswap login {arrived}"
        );
    }

    if fresh_profile {
        ensure_account_free(&cfg, &email)?;
        cfg.profiles.push(Profile::new(email.clone()));
        cfg.save()?;
        println!("Created profile for {email}");
        if profile::live_email().as_deref() == Some(email.as_str()) {
            println!(
                "note: {email} is also the default (~/.claude). Separate logins, \
                 separate tokens — that's the one overlap cswap allows."
            );
        }
    } else {
        println!("Logged {email} in again.");
    }
    profile::harden_identity(&email)?;

    match alias_arg {
        Some(alias) => add_alias(&mut cfg, &email, &alias)?,
        // Offer an alias right away — skippable with Enter.
        None if interactive::on_tty() && fresh_profile => {
            if let Some(alias) =
                interactive::input_optional(&format!("Alias for {email} (Enter to skip)"))?
            {
                add_alias(&mut cfg, &email, &alias)?;
            }
        }
        None => {}
    }
    Ok(())
}

/// A captured credential set must carry a refresh token, and the profile's
/// .claude.json must name who logged in. Returns that account.
fn verify_login(creds: &std::path::Path, email: &str) -> Result<String> {
    let creds_text = fs::read_to_string(creds)
        .map_err(|_| anyhow::anyhow!("no login captured — claude exited without completing one"))?;
    let value: serde_json::Value =
        serde_json::from_str(&creds_text).context(".credentials.json is not valid JSON")?;
    let oauth = value
        .get("claudeAiOauth")
        .and_then(serde_json::Value::as_object)
        .context("credentials carry no claudeAiOauth — the login did not complete")?;
    if oauth
        .get("refreshToken")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .is_empty()
    {
        bail!("credentials have no refreshToken — run `cswap login {email}` again");
    }
    profile::recorded_email(email).context(
        "the login completed but claude recorded no account email — \
         run `cswap login` again and let claude finish starting up",
    )
}

fn add_alias(cfg: &mut Config, email: &str, alias: &str) -> Result<()> {
    if !valid_label(alias) {
        bail!("invalid alias '{alias}' (use lowercase letters, digits, '-', '_', '.')");
    }
    if cfg.label_taken(alias) {
        bail!("'{alias}' is already used as an alias or account");
    }
    cfg.profiles
        .iter_mut()
        .find(|p| p.email == email)
        .expect("caller resolved")
        .aliases
        .push(alias.to_string());
    cfg.save()?;
    println!("'{alias}' now points to {email}.");
    Ok(())
}

/// Remove a profile's directory. Its symlinks are removed WITHOUT following
/// them, so the real ~/.claude data behind them is untouched.
pub fn remove_dir(email: &str) -> Result<()> {
    let dir = paths::profile_dir(email);
    if dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("failed to remove {}", dir.display()))?;
    }
    Ok(())
}
