//! Per-account profile directories (the CLAUDE_CONFIG_DIR targets).
//!
//! A profile is ~/.claude wearing a different identity card:
//!   .credentials.json   real file — this account's tokens (0600)
//!   .claude.json        real file — oauthAccount + onboarding seed (0600)
//!   <everything else>   symlink into ~/.claude, auto-discovered per launch
//!
//! Safety contract: this module NEVER writes into ~/.claude or ~/.claude.json.
//! It reads them; all writes land under ~/.local/share/cswap/.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::config::Account;
use crate::{oauth, paths};

/// Never linked — identity stays per-profile. (.claude.json lives in $HOME,
/// not inside ~/.claude, so it never appears in the scan at all.)
const DENYLIST: &[&str] = &[".credentials.json"];

/// Additionally skipped for `isolated = true` accounts.
const HISTORY_ITEMS: &[&str] = &["projects", "history.jsonl"];

/// Keys copied once from the live ~/.claude.json into a fresh profile's
/// .claude.json: user-scope MCP servers and per-project trust/allowlists.
const SEEDED_KEYS: &[&str] = &["mcpServers", "projects"];

/// Email of the identity currently logged into the live ~/.claude, if any.
/// Accounts matching it run via passthrough (no profile, no token handling)
/// so cswap can never rotate the live login's refresh-token family.
pub fn live_email() -> Option<String> {
    let live: Value = serde_json::from_str(&fs::read_to_string(paths::claude_json()).ok()?).ok()?;
    live.get("oauthAccount")?
        .get("emailAddress")?
        .as_str()
        .map(String::from)
}

pub fn write_private(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Load this account's current credentials (profile copy if the profile
/// exists — it is the live store — else the login-time backup), refresh the
/// token if it is about to expire, and persist any rotation everywhere.
pub fn current_creds(acct: &Account) -> Result<Value> {
    let profile_creds = paths::profile_dir(&acct.name).join(".credentials.json");
    let store = paths::store_creds(&acct.name);
    let source = if profile_creds.exists() {
        &profile_creds
    } else {
        &store
    };
    let text = fs::read_to_string(source).with_context(|| {
        format!(
            "no stored credentials for '{}' — log in with `claude`, then run `cswap login`",
            acct.name
        )
    })?;
    let mut creds: Value = serde_json::from_str(&text)
        .with_context(|| format!("malformed credentials: {}", source.display()))?;
    if oauth::refresh_if_needed(&mut creds, oauth::REFRESH_MARGIN_MS)? {
        let serialized = serde_json::to_string(&creds)?;
        write_private(&store, &serialized)?;
        if profile_creds.exists() {
            write_private(&profile_creds, &serialized)?;
        }
    }
    Ok(creds)
}

/// Make the profile launch-ready and return its path.
pub fn ensure(acct: &Account) -> Result<PathBuf> {
    let dir = paths::profile_dir(&acct.name);
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;

    // Credentials: seed from the store on first run; afterwards the profile
    // copy is the source of truth (Claude rotates tokens in place there).
    let creds_path = dir.join(".credentials.json");
    if !creds_path.exists() {
        let stored = fs::read_to_string(paths::store_creds(&acct.name)).with_context(|| {
            format!(
                "no stored credentials for '{}' — log in with `claude`, then run `cswap login`",
                acct.name
            )
        })?;
        write_private(&creds_path, &stored)?;
    }
    let creds = current_creds(acct)?; // refreshes + persists if stale
                                      // current_creds may have refreshed only the store if it raced dir creation;
                                      // make sure the profile copy is what we resolved.
    write_private(&creds_path, &serde_json::to_string(&creds)?)?;

    let claude_json = dir.join(".claude.json");
    if !claude_json.exists() {
        seed_claude_json(&claude_json, acct)?;
    }

    sync_links(&dir, acct.isolated)?;
    Ok(dir)
}

/// First-launch .claude.json: identity + the two keys that skip onboarding
/// (`theme` is load-bearing: Claude shows the wizard when theme or
/// hasCompletedOnboarding is missing), plus one-time copies of user-scope
/// MCP servers and per-project trust from the live config.
fn seed_claude_json(path: &Path, acct: &Account) -> Result<()> {
    let meta: Value = fs::read_to_string(paths::store_meta(&acct.name))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    let live: Value = fs::read_to_string(paths::claude_json())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));

    let mut out = json!({ "hasCompletedOnboarding": true });
    let theme = meta
        .get("theme")
        .or_else(|| live.get("theme"))
        .and_then(Value::as_str)
        .unwrap_or("dark");
    out["theme"] = json!(theme);
    if let Some(oa) = meta.get("oauthAccount") {
        out["oauthAccount"] = oa.clone();
    }
    for key in SEEDED_KEYS {
        if let Some(v) = live.get(*key) {
            out[*key] = v.clone();
        }
    }
    write_private(path, &serde_json::to_string_pretty(&out)?)
}

/// Mirror ~/.claude into the profile as symlinks: everything except the
/// denylist (and history items for isolated accounts). Re-run every launch so
/// files Claude Code invents in future versions are picked up automatically.
/// Existing real files in the profile are left untouched; dangling links into
/// ~/.claude are pruned.
pub fn sync_links(profile: &Path, isolated: bool) -> Result<()> {
    let src_root = paths::claude_dir();
    if !src_root.is_dir() {
        return Ok(()); // nothing to share yet
    }

    // Prune dangling cswap-made links (target vanished from ~/.claude).
    for entry in fs::read_dir(profile)? {
        let entry = entry?;
        let link = entry.path();
        if entry.file_type()?.is_symlink() {
            if let Ok(target) = fs::read_link(&link) {
                if target.starts_with(&src_root) && !target.exists() {
                    fs::remove_file(&link)?;
                }
            }
        }
    }

    for entry in fs::read_dir(&src_root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if DENYLIST.contains(&name_str.as_ref()) {
            continue;
        }
        if isolated && HISTORY_ITEMS.contains(&name_str.as_ref()) {
            continue;
        }
        let target = src_root.join(&name);
        let link = profile.join(&name);
        match fs::symlink_metadata(&link) {
            Ok(md) if md.file_type().is_symlink() => {
                if fs::read_link(&link).map(|t| t != target).unwrap_or(true) {
                    fs::remove_file(&link)?;
                    symlink(&target, &link)?;
                }
            }
            Ok(_) => {} // real file/dir the profile grew on its own — never clobber
            Err(_) => symlink(&target, &link)?,
        }
    }

    // Flipping an account to isolated: drop previously-created history links.
    if isolated {
        for item in HISTORY_ITEMS {
            let link = profile.join(item);
            if fs::symlink_metadata(&link)
                .map(|md| md.file_type().is_symlink())
                .unwrap_or(false)
            {
                fs::remove_file(&link)?;
            }
        }
    }
    Ok(())
}
