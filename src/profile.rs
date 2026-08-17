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

use anyhow::{bail, Context, Result};
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

/// Has this profile ever completed a login? Callers use it to skip the usage
/// API entirely — asking an endpoint about an account with no token wastes a
/// request against a per-token hourly budget and returns a worse message than
/// "not logged in".
pub fn has_login(email: &str) -> bool {
    creds_path(email).exists()
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
    // Checked before reading so the message has no `os error 2` hanging off it.
    // "No such file or directory" is not the useful half of "not logged in".
    if !path.exists() {
        bail!("not logged in — run: cswap login {}", prof.email);
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
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
    warn_shadows(&prof.email, &sync_links(&dir)?);
    Ok(dir)
}

/// Report shadowed names on stderr. Every launch, every time — a shadow means
/// data is being written where nothing else can see it, so silence is the one
/// response that is always wrong.
pub fn warn_shadows(email: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    eprintln!(
        "cswap: warning — profile {email} has real files where share links belong: {}",
        names.join(", ")
    );
    eprintln!(
        "cswap: anything written there is invisible to your other profiles and to ~/.claude."
    );
    eprintln!("cswap: inspect with `cswap doctor`, fix with `cswap doctor --repair`.");
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

/// Point `link` at `target` without ever leaving the path absent.
///
/// The obvious `remove_file` then `symlink` opens a window — microseconds, but
/// a claude process that resolves the path inside it finds nothing and creates
/// a REAL directory there, which then shadows the share permanently. That is
/// how a live session's transcripts end up written into a profile instead of
/// ~/.claude. `rename` over an existing symlink is atomic, so there is no
/// window at all.
fn link_atomic(target: &Path, link: &Path) -> Result<()> {
    let name = link.file_name().unwrap_or_default().to_string_lossy();
    let tmp = link.with_file_name(format!(".cswap-link.{name}"));
    let _ = fs::remove_file(&tmp);
    symlink(target, &tmp)
        .with_context(|| format!("failed to stage a link at {}", tmp.display()))?;
    fs::rename(&tmp, link).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// Names in `profile` that hold a REAL file or directory where a share link
/// belongs — the name exists in ~/.claude and is not denylisted.
///
/// This is the failure that costs data. Claude writes to whatever the path
/// resolves to, so a real `projects/` here means transcripts land in the
/// profile and are invisible to every other entity, with nothing reporting it.
/// Read-only: finding a shadow never fixes it, because the fix has to decide
/// what happens to the files inside.
pub fn shadows(profile: &Path) -> Vec<String> {
    let src_root = paths::claude_dir();
    let Ok(entries) = fs::read_dir(&src_root) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();
            if DENYLIST.contains(&name_str.as_str()) {
                return None;
            }
            let link = profile.join(&name);
            match fs::symlink_metadata(&link) {
                Ok(md) if !md.file_type().is_symlink() => Some(name_str),
                _ => None,
            }
        })
        .collect();
    found.sort();
    found
}

/// Mirror ~/.claude into the profile as symlinks: everything except the
/// denylist. Re-run every launch so files Claude Code invents in future
/// versions are picked up automatically. Stale links are pruned.
///
/// Returns the names it could NOT link because a real file or directory holds
/// the spot. They are never clobbered — the files inside may be the only copy —
/// but they are never swallowed either. Callers must report them.
pub fn sync_links(profile: &Path) -> Result<Vec<String>> {
    let src_root = paths::claude_dir();
    if !src_root.is_dir() {
        return Ok(Vec::new()); // nothing to share yet
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

    let mut shadowed = Vec::new();
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
                    link_atomic(&target, &link)?;
                }
            }
            // A real file or directory the profile grew on its own. Never
            // clobber it — but say so, every time.
            Ok(_) => shadowed.push(name_str.to_string()),
            Err(_) => link_atomic(&target, &link)?,
        }
    }
    shadowed.sort();
    Ok(shadowed)
}

/// PIDs of claude processes running with `dir` as their CLAUDE_CONFIG_DIR.
///
/// Rearranging a profile under a live session is what broke this before: the
/// process holds the config dir as a path string and re-resolves it on every
/// write, so a link that disappears for an instant becomes a real directory.
/// Anything that prunes, moves or removes a profile checks this first.
///
/// Linux only. /proc/<pid>/environ has no portable equivalent, so on other
/// platforms this reports nothing and the callers fall back to asking.
#[cfg(target_os = "linux")]
pub fn live_sessions(dir: &Path) -> Vec<u32> {
    let needle = format!("CLAUDE_CONFIG_DIR={}", dir.display());
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut pids: Vec<u32> = entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_string_lossy().parse::<u32>().ok()?;
            // Unreadable /proc entries are other users' processes, not ours.
            let environ = fs::read(entry.path().join("environ")).ok()?;
            environ
                .split(|b| *b == 0)
                .any(|var| var == needle.as_bytes())
                .then_some(pid)
        })
        .collect();
    pids.sort_unstable();
    pids
}

#[cfg(not(target_os = "linux"))]
pub fn live_sessions(_dir: &Path) -> Vec<u32> {
    Vec::new()
}
