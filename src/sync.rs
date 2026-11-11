//! Mirror a harness's native config dir into an account's `config_dir`.
//!
//! An account's `config_dir` holds its own credentials as real files and a
//! symlink to the native dir for everything else, so settings, skills, plugins
//! and history are one copy shared by every account. The list is rebuilt from
//! what is in the native dir on every sync, so a file a future version of the
//! agent invents is shared without a code change.
//!
//! Rules that do not bend:
//! - cswap never writes into a native dir.
//! - A real file in the way of a link is never deleted. If it is identical to
//!   the native one it is replaced by the link; otherwise it is reported, and
//!   `repair` moves it to `~/.cswap/parked/` before linking.

use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::harness::Harness;
use crate::store::{Account, HarnessSettings};
use crate::{fsx, paths};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// A link to the native entry, as it should be.
    Linked,
    /// A name that would be linked, but is not yet.
    Missing,
    /// A link that points somewhere else.
    Wrong,
    /// A real file or directory sits where the link belongs.
    Shadowed,
    /// Kept private to the account on purpose.
    Private,
    /// A link whose native target has gone.
    Stale,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub state: State,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub entries: Vec<Entry>,
}

impl Report {
    pub fn count(&self, state: State) -> usize {
        self.entries.iter().filter(|e| e.state == state).count()
    }

    pub fn names(&self, state: State) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.state == state)
            .map(|e| e.name.clone())
            .collect()
    }

    /// Nothing to do and nothing in the way.
    pub fn clean(&self) -> bool {
        self.entries
            .iter()
            .all(|e| matches!(e.state, State::Linked | State::Private))
    }

    /// Entries a plain sync would fix on its own.
    pub fn pending(&self) -> usize {
        self.count(State::Missing) + self.count(State::Wrong) + self.count(State::Stale)
    }
}

/// Is `rel` (a path relative to the config dir) kept out of the mirror?
fn is_private(h: Harness, extra: &[String], rel: &str) -> bool {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    h.is_private(rel)
        || extra.iter().any(|e| e == rel)
        // cswap's own staging names, lock files and atomic-write leftovers. A
        // link to another process's lock is never what anyone wants.
        || base.starts_with(".cswap-")
        || base.ends_with(".lock")
        || base.contains(".tmp.")
}

fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// Compare one directory level. `prefix` is empty for the top level, or the
/// relative path of a nested dir that is mirrored entry by entry.
fn walk(
    h: Harness,
    native: &Path,
    config: &Path,
    prefix: &str,
    extra: &[String],
    out: &mut Vec<Entry>,
) {
    let (ndir, cdir) = (native.join(prefix), config.join(prefix));
    let rel_of = |name: &str| {
        if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        }
    };
    let mut shared = HashSet::new();

    if let Ok(rd) = fs::read_dir(&ndir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let rel = rel_of(&name);
            let is_dir = e.path().is_dir();
            let link = cdir.join(&name);
            let push = |out: &mut Vec<Entry>, state| {
                out.push(Entry {
                    name: rel.clone(),
                    state,
                    is_dir,
                })
            };
            if is_private(h, extra, &rel) {
                // A private name must never be a link into the native dir.
                let leaked =
                    is_link(&link) && fs::read_link(&link).is_ok_and(|t| t.starts_with(native));
                push(out, if leaked { State::Stale } else { State::Private });
                continue;
            }
            shared.insert(name.clone());
            if is_dir && h.spec().nested.contains(&rel.as_str()) {
                // Mirrored one level down: it must be a real directory here.
                match fs::symlink_metadata(&link) {
                    Ok(md) if md.file_type().is_symlink() => push(out, State::Wrong),
                    Ok(md) if !md.is_dir() => push(out, State::Shadowed),
                    _ => walk(h, native, config, &rel, extra, out),
                }
                continue;
            }
            let state = match fs::symlink_metadata(&link) {
                Ok(md) if md.file_type().is_symlink() => match fs::read_link(&link) {
                    Ok(t) if t == ndir.join(&name) => State::Linked,
                    _ => State::Wrong,
                },
                Ok(_) => State::Shadowed,
                Err(_) => State::Missing,
            };
            push(out, state);
        }
    }

    // Links into the native dir whose target has gone.
    if let Ok(rd) = fs::read_dir(&cdir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if shared.contains(&name) || out.iter().any(|x| x.name == rel_of(&name)) {
                continue;
            }
            if is_link(&e.path()) && fs::read_link(e.path()).is_ok_and(|t| t.starts_with(native)) {
                out.push(Entry {
                    name: rel_of(&name),
                    state: State::Stale,
                    is_dir: false,
                });
            }
        }
    }
}

/// Compare an account's `config_dir` against the native dir. Read-only.
pub fn check(h: Harness, config_dir: &Path, extra_private: &[String]) -> Report {
    let mut entries = Vec::new();
    walk(
        h,
        &h.native_dir(),
        config_dir,
        "",
        extra_private,
        &mut entries,
    );
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Report { entries }
}

/// Are two paths the same content? Files are compared byte for byte; anything
/// else (directories, unreadable files) is "not known to be the same".
fn identical(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (fs::metadata(a), fs::metadata(b)) else {
        return false;
    };
    if !ma.is_file() || !mb.is_file() || ma.len() != mb.len() {
        return false;
    }
    match (fs::read(a), fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn empty_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
        && fs::read_dir(p).is_ok_and(|mut rd| rd.next().is_none())
}

/// Bring `config_dir` in line with the native dir and return what is left.
///
/// Creates missing links, retargets wrong ones and prunes stale ones. A
/// shadowing file is replaced only when that loses nothing: it is identical to
/// the native file, or it is an empty directory. Everything else stays and is
/// reported as `Shadowed`.
pub fn apply(h: Harness, config_dir: &Path, extra_private: &[String]) -> Result<Report> {
    let native = h.native_dir();
    if !native.is_dir() {
        return Ok(Report::default());
    }
    fsx::private_dir(config_dir)?;
    // A nested dir that was a whole-dir link becomes a real dir in one pass and
    // gets its own entries linked in the next, hence the short loop.
    for _ in 0..=h.spec().nested.len() {
        let report = check(h, config_dir, extra_private);
        if report.pending() == 0 && report.count(State::Shadowed) == 0 {
            return Ok(report);
        }
        for e in &report.entries {
            let (target, link) = (native.join(&e.name), config_dir.join(&e.name));
            let nested = h.spec().nested.contains(&e.name.as_str());
            match e.state {
                State::Wrong if nested => {
                    crate::platform::remove_link(&link)?;
                    fsx::private_dir(&link)?;
                }
                State::Missing | State::Wrong => {
                    if let Some(parent) = link.parent() {
                        fsx::private_dir(parent)?;
                    }
                    fsx::link_atomic(&target, &link)
                        .with_context(|| format!("cannot link {}", e.name))?
                }
                State::Stale => {
                    let _ = crate::platform::remove_link(&link);
                }
                State::Shadowed => {
                    // Safe to replace: a copy of the shared file, or an empty dir.
                    let spare = identical(&link, &target)
                        || (!nested && empty_dir(&link) && fs::remove_dir(&link).is_ok());
                    if spare {
                        fsx::link_atomic(&target, &link)?;
                    }
                }
                State::Private | State::Linked => {}
            }
        }
    }
    Ok(check(h, config_dir, extra_private))
}

/// Move every shadowing entry into `~/.cswap/parked/<key>/<time>/` and put the
/// link back. Nothing is merged into the native dir and nothing is deleted;
/// the returned path is where the files went.
pub fn repair(
    h: Harness,
    key: &str,
    config_dir: &Path,
    extra: &[String],
) -> Result<Option<PathBuf>> {
    let native = h.native_dir();
    let shadowed = apply(h, config_dir, extra)?.names(State::Shadowed);
    if shadowed.is_empty() {
        return Ok(None);
    }
    let stamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let park = paths::parked_dir(key).join(stamp);
    fsx::private_dir(&park)?;
    for name in &shadowed {
        let (from, to) = (config_dir.join(name), park.join(name));
        if let Some(parent) = to.parent() {
            fsx::private_dir(parent)?;
        }
        fs::rename(&from, &to).with_context(|| format!("cannot move {} aside", from.display()))?;
        fsx::link_atomic(&native.join(name), &from)?;
    }
    Ok(Some(park))
}

/// Sync one account and record the outcome in its settings. Native accounts
/// have nothing to sync.
pub fn sync_account(acc: &mut Account, hs: &HarnessSettings) -> Result<Report> {
    if acc.is_native() {
        return Ok(Report::default());
    }
    let report = apply(acc.harness, &acc.config_dir(), &hs.sync.exclude)?;
    record(acc, &report, true)?;
    Ok(report)
}

/// Check one account without changing its links, and record what was seen.
pub fn check_account(acc: &mut Account, hs: &HarnessSettings) -> Result<Report> {
    if acc.is_native() {
        return Ok(Report::default());
    }
    let report = check(acc.harness, &acc.config_dir(), &hs.sync.exclude);
    record(acc, &report, false)?;
    Ok(report)
}

fn record(acc: &mut Account, report: &Report, synced: bool) -> Result<()> {
    let now = Utc::now();
    let (linked, private, shadowed) = (
        report.count(State::Linked),
        report.count(State::Private),
        report.names(State::Shadowed),
    );
    acc.update(|m| {
        m.sync.last_checked = Some(now);
        if synced {
            m.sync.last_synced = Some(now);
        }
        m.sync.linked = linked;
        m.sync.private = private;
        m.sync.shadowed = shadowed;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Sandbox;

    #[test]
    fn mirrors_everything_but_private_names() {
        let sb = Sandbox::new();
        let native = sb.native(Harness::Claude);
        fs::create_dir_all(native.join("projects")).unwrap();
        fs::write(native.join("settings.json"), "{}").unwrap();
        fs::write(native.join(".credentials.json"), "secret").unwrap();
        let dir = sb.root().join("acc/config_dir");

        let report = apply(Harness::Claude, &dir, &[]).unwrap();
        assert!(report.clean(), "{report:?}");
        assert!(fs::symlink_metadata(dir.join("projects"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(dir.join("settings.json")).unwrap(),
            native.join("settings.json")
        );
        assert!(!dir.join(".credentials.json").exists());
        assert_eq!(report.count(State::Private), 1);
    }

    #[test]
    fn a_differing_shadow_is_kept_and_reported_then_parked_by_repair() {
        let sb = Sandbox::new();
        let native = sb.native(Harness::Claude);
        fs::write(native.join("settings.json"), "native").unwrap();
        fs::write(native.join("same.json"), "same").unwrap();
        let dir = sb.root().join("acc/config_dir");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("settings.json"), "account wrote this").unwrap();
        fs::write(dir.join("same.json"), "same").unwrap();

        let report = apply(Harness::Claude, &dir, &[]).unwrap();
        // The identical file became a link; the differing one did not.
        assert_eq!(report.names(State::Shadowed), vec!["settings.json"]);
        assert!(fs::symlink_metadata(dir.join("same.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(dir.join("settings.json")).unwrap(),
            "account wrote this"
        );

        let park = repair(Harness::Claude, "claude-1", &dir, &[])
            .unwrap()
            .expect("something was parked");
        assert_eq!(
            fs::read_to_string(park.join("settings.json")).unwrap(),
            "account wrote this"
        );
        assert_eq!(
            fs::read_to_string(dir.join("settings.json")).unwrap(),
            "native"
        );
        assert_eq!(
            fs::read_to_string(native.join("settings.json")).unwrap(),
            "native"
        );
    }

    #[test]
    fn stale_links_are_pruned_and_new_names_picked_up() {
        let sb = Sandbox::new();
        let native = sb.native(Harness::Claude);
        fs::write(native.join("old.json"), "x").unwrap();
        let dir = sb.root().join("acc/config_dir");
        apply(Harness::Claude, &dir, &[]).unwrap();

        fs::remove_file(native.join("old.json")).unwrap();
        fs::write(native.join("new.json"), "y").unwrap();
        assert_eq!(check(Harness::Claude, &dir, &[]).pending(), 2);
        let report = apply(Harness::Claude, &dir, &[]).unwrap();
        assert!(report.clean());
        assert!(fs::symlink_metadata(dir.join("old.json")).is_err());
        assert!(dir.join("new.json").exists());
    }

    #[test]
    fn a_nested_dir_is_mirrored_entry_by_entry_with_its_private_names_left_out() {
        let sb = Sandbox::new();
        let native = sb.native(Harness::Agy);
        let cli = native.join("antigravity-cli");
        fs::create_dir_all(cli.join("conversations")).unwrap();
        fs::create_dir_all(cli.join("cache")).unwrap();
        fs::write(cli.join("settings.json"), "{}").unwrap();
        fs::write(cli.join("antigravity-oauth-token"), "secret").unwrap();
        fs::write(cli.join("cache/onboarding.json"), "{}").unwrap();
        fs::write(cli.join("cache/last_conversations.json"), "[]").unwrap();
        fs::write(native.join("AGENTS.md"), "rules").unwrap();
        let dir = sb.root().join("acc/config_dir");

        let report = apply(Harness::Agy, &dir, &[]).unwrap();
        assert!(report.clean(), "{report:?}");
        let link = |p: &str| {
            fs::symlink_metadata(dir.join(p))
                .unwrap()
                .file_type()
                .is_symlink()
        };
        assert!(link("AGENTS.md"));
        assert!(!link("antigravity-cli"), "a nested dir is a real dir");
        assert!(link("antigravity-cli/settings.json"));
        assert!(link("antigravity-cli/conversations"));
        assert!(!link("antigravity-cli/cache"));
        assert!(link("antigravity-cli/cache/last_conversations.json"));
        assert!(!dir.join("antigravity-cli/antigravity-oauth-token").exists());
        assert!(!dir.join("antigravity-cli/cache/onboarding.json").exists());
    }

    #[test]
    fn a_whole_dir_link_where_a_nested_dir_belongs_is_taken_apart() {
        let sb = Sandbox::new();
        let native = sb.native(Harness::Agy);
        let cli = native.join("antigravity-cli");
        fs::create_dir_all(&cli).unwrap();
        fs::write(cli.join("settings.json"), "{}").unwrap();
        fs::write(cli.join("antigravity-oauth-token"), "secret").unwrap();
        let dir = sb.root().join("acc/config_dir");
        fs::create_dir_all(&dir).unwrap();
        crate::platform::symlink(&cli, &dir.join("antigravity-cli")).unwrap();

        let report = apply(Harness::Agy, &dir, &[]).unwrap();
        assert!(report.clean(), "{report:?}");
        assert!(fs::symlink_metadata(dir.join("antigravity-cli"))
            .unwrap()
            .is_dir());
        // The token is no longer reachable through the account dir.
        assert!(!dir.join("antigravity-cli/antigravity-oauth-token").exists());
        assert!(
            cli.join("antigravity-oauth-token").exists(),
            "native untouched"
        );
    }

    #[test]
    fn user_excludes_stay_private() {
        let sb = Sandbox::new();
        let native = sb.native(Harness::Claude);
        fs::write(native.join("keep-out"), "x").unwrap();
        let dir = sb.root().join("acc/config_dir");
        let report = apply(Harness::Claude, &dir, &["keep-out".to_string()]).unwrap();
        assert_eq!(report.count(State::Private), 1);
        assert!(!dir.join("keep-out").exists());
    }
}
