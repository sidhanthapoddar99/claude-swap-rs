//! Profile directories (the CLAUDE_CONFIG_DIR targets).
//!
//! A profile is ~/.claude wearing a different identity card:
//!   .credentials.json   real file — THIS profile's tokens, the only copy (0600)
//!   .claude.json        real file — oauthAccount + onboarding seed (0600)
//!   <DENYLIST>          never linked — see the constant below
//!   <everything else>   symlink into ~/.claude, auto-discovered per launch
//!
//! Safety contract: cswap never writes into ~/.claude or ~/.claude.json. The
//! live directory is the `default` entity and it is read-only to us — we read
//! it to report who is logged in, to seed a new profile's theme and trust, and
//! to mirror it as symlinks. Every cswap write lands under
//! ~/.local/share/cswap/ or ~/.config/cswap/.
//!
//! Nothing here compares a profile against the live login. The default may
//! hold the same account as a profile — it is the only thing allowed to — and
//! each keeps its own token family, which is only true as long as no code path
//! copies credentials between them.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::config::Profile;
use crate::{oauth, paths};

/// Never linked. (.claude.json lives in $HOME, not inside ~/.claude, so it
/// never appears in the scan at all.)
///
///   .credentials.json  identity — this profile's own tokens.
///   backups            holds `.claude.json.backup.<ms>` snapshots. `.claude.json`
///                      is per-profile, so its backups must be too: a shared
///                      directory keyed only by timestamp lets a profile restore
///                      the LIVE account's identity file.
///   .git               the user may version-control ~/.claude. A linked `.git`
///                      makes git treat the PROFILE as that repo's working tree,
///                      so every real file reads as deleted or type-changed. One
///                      `git add -A` inside the profile rewrites ~/.claude's
///                      tracked tree; one `git checkout` materialises real files
///                      that then permanently shadow the share links.
const DENYLIST: &[&str] = &[".credentials.json", "backups", ".git"];

/// Keys copied once from the live ~/.claude.json into a fresh profile's
/// .claude.json: user-scope MCP servers and per-project trust/allowlists.
/// Convenience only — none of this is a credential.
const SEEDED_KEYS: &[&str] = &["mcpServers", "projects"];

/// Email of the identity currently logged into the live ~/.claude, if any.
/// Display only: this answers "who is `default`", and nothing branches on it.
pub fn live_email() -> Option<String> {
    let live: Value = serde_json::from_str(&fs::read_to_string(paths::claude_json()).ok()?).ok()?;
    live.get("oauthAccount")?
        .get("emailAddress")?
        .as_str()
        .map(String::from)
}

/// The live ~/.claude token, read as-is. Never refreshed: rotating the live
/// login's token family is claude's job, not ours.
pub fn live_creds() -> Result<Value> {
    let text = fs::read_to_string(paths::live_credentials())
        .context("no live ~/.claude login — run `claude` and log in")?;
    let creds: Value = serde_json::from_str(&text).context("malformed ~/.claude credentials")?;
    let fresh = creds
        .get("claudeAiOauth")
        .and_then(|o| o.get("expiresAt"))
        .and_then(Value::as_i64)
        .is_some_and(|t| t > oauth::now_ms());
    if !fresh {
        anyhow::bail!("live token expired — run claude once to refresh");
    }
    Ok(creds)
}

pub fn write_private(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub fn creds_path(email: &str) -> PathBuf {
    paths::profile_dir(email).join(".credentials.json")
}

/// Re-assert 0600 on the profile's two identity files.
///
/// claude writes both itself during a login, and it replaces `.claude.json` by
/// rename — so whatever mode cswap set when seeding is gone and the result
/// follows claude's umask. `.credentials.json` is now the only copy of this
/// profile's tokens, so its mode is not something to inherit from elsewhere.
pub fn harden_identity(email: &str) -> Result<()> {
    let dir = paths::profile_dir(email);
    for file in [".credentials.json", ".claude.json"] {
        let path = dir.join(file);
        if path.exists() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .with_context(|| format!("failed to secure {}", path.display()))?;
        }
    }
    Ok(())
}

/// Load this profile's credentials, refresh the token if it is about to expire,
/// and persist any rotation. The profile directory holds the only copy, so
/// there is nowhere else to write it back to.
pub fn current_creds(prof: &Profile) -> Result<Value> {
    let path = creds_path(&prof.email);
    let text = fs::read_to_string(&path).with_context(|| {
        format!(
            "profile {} has no login yet — run `cswap login {}`",
            prof.email, prof.email
        )
    })?;
    let mut creds: Value = serde_json::from_str(&text)
        .with_context(|| format!("malformed credentials: {}", path.display()))?;
    if oauth::refresh_if_needed(&mut creds, oauth::REFRESH_MARGIN_MS)? {
        write_private(&path, &serde_json::to_string(&creds)?)?;
    }
    Ok(creds)
}

/// Create the profile directory and its share links, without requiring a
/// login. This is what `cswap login` prepares before handing the directory to
/// claude: the links must exist BEFORE claude runs, or claude creates real
/// directories that then permanently shadow them.
pub fn scaffold(prof: &Profile) -> Result<PathBuf> {
    let dir = paths::profile_dir(&prof.email);
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let claude_json = dir.join(".claude.json");
    if !claude_json.exists() {
        seed_claude_json(&claude_json)?;
    }
    sync_links(&dir)?;
    Ok(dir)
}

/// Make the profile launch-ready and return its path. Fails when the profile
/// has never completed a login — there is no store to fall back on.
pub fn ensure(prof: &Profile) -> Result<PathBuf> {
    let dir = scaffold(prof)?;
    let creds = current_creds(prof)?; // refreshes + persists if stale
    write_private(&creds_path(&prof.email), &serde_json::to_string(&creds)?)?;
    // The previous session may have left .claude.json at claude's umask.
    harden_identity(&prof.email)?;
    Ok(dir)
}

/// Read the email a completed login recorded in the profile's .claude.json.
pub fn recorded_email(email: &str) -> Option<String> {
    let text = fs::read_to_string(paths::profile_dir(email).join(".claude.json")).ok()?;
    let cj: Value = serde_json::from_str(&text).ok()?;
    cj.get("oauthAccount")?
        .get("emailAddress")?
        .as_str()
        .map(String::from)
}

/// First-launch .claude.json: the two keys that skip onboarding (`theme` is
/// load-bearing: Claude shows the wizard when theme or hasCompletedOnboarding
/// is missing), plus one-time copies of user-scope MCP servers and per-project
/// trust from the live config.
///
/// No identity is written. Claude records `oauthAccount` itself when the
/// profile's own login completes — copying it in would describe a login that
/// this profile has not performed.
fn seed_claude_json(path: &Path) -> Result<()> {
    let live: Value = fs::read_to_string(paths::claude_json())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));

    let mut out = json!({ "hasCompletedOnboarding": true });
    out["theme"] = json!(live.get("theme").and_then(Value::as_str).unwrap_or("dark"));
    for key in SEEDED_KEYS {
        if let Some(v) = live.get(*key) {
            out[*key] = v.clone();
        }
    }
    write_private(path, &serde_json::to_string_pretty(&out)?)
}

/// Mirror ~/.claude into the profile as symlinks: everything except the
/// denylist. Re-run every launch so files Claude Code invents in future
/// versions are picked up automatically. Existing real files in the profile
/// are left untouched; stale links are pruned.
pub fn sync_links(profile: &Path) -> Result<()> {
    let src_root = paths::claude_dir();
    if !src_root.is_dir() {
        return Ok(()); // nothing to share yet
    }

    // Prune cswap-made links that should no longer exist: the target vanished
    // from ~/.claude, or the name has since joined the denylist (so a profile
    // built by an older cswap repairs itself). Real files are never touched.
    for entry in fs::read_dir(profile)? {
        let entry = entry?;
        let link = entry.path();
        if entry.file_type()?.is_symlink() {
            if let Ok(target) = fs::read_link(&link) {
                let denied = DENYLIST.contains(&entry.file_name().to_string_lossy().as_ref());
                if target.starts_with(&src_root) && (denied || !target.exists()) {
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
    Ok(())
}
