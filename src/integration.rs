//! Integrations: pieces of cswap for other programs.
//!
//! They live in this repository under `integrations/`, not in the binary. An
//! install fetches the files that go with this version of cswap and puts them
//! where the other program reads them. There is one so far: a layout for
//! herdr.
//!
//! Three rules keep an install safe to run again and again:
//! - every file is read before one is written, so a failed download changes
//!   nothing;
//! - a file cswap did not write, or that was edited since, is set aside as
//!   `<file>.mine`, never overwritten;
//! - only `layouts/cswap.*` and `mods/cswap-*` are ever touched.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::{fsx, http, paths, update};

/// What the Integrations page says about one of them.
pub struct Integration {
    pub name: &'static str,
    pub blurb: &'static str,
    /// What it adds, one line each.
    pub adds: &'static [&'static str],
}

pub const HERDR: Integration = Integration {
    name: "herdr",
    blurb: "a layout that puts cswap's accounts into herdr",
    adds: &[
        "+ menu: start an agent on the account you pick",
        "right-click an agent: move it to another account, same conversation",
        "Ctrl+click agents: move or close several at once",
        "right-click an agent: start a sub-agent under it",
        "agents list: show each agent's account",
        "sidebar: this dashboard in a popup",
    ],
};

pub const ALL: [&Integration; 1] = [&HERDR];

/// The layout's name in herdr.
pub const LAYOUT: &str = "cswap";

/// The list of files an integration is made of.
#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    files: Vec<String>,
}

/// What an install wrote, kept so the next one can tell its own files from
/// ones the user changed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Record {
    pub version: String,
    /// The cswap that installed it.
    pub cswap: String,
    pub installed_at: DateTime<Utc>,
    /// Each file and the SHA-256 of what was written.
    pub files: BTreeMap<String, String>,
}

fn record_path() -> PathBuf {
    paths::root().join("integrations").join("herdr.json")
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// herdr's own folder. `HERDR_APP_HOME` moves it, as it does for herdr.
pub fn herdr_home() -> PathBuf {
    match std::env::var_os("HERDR_APP_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => paths::home().join(".config").join("herdr-app"),
    }
}

/// May an install write this file? Only the layout's own files and the mods
/// named after cswap, and nothing that climbs out of herdr's folder.
fn allowed(path: &str) -> bool {
    let plain = path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c));
    let inside = Path::new(path)
        .components()
        .all(|c| matches!(c, Component::Normal(_)));
    let parts: Vec<&str> = path.split('/').collect();
    let ours = match parts.as_slice() {
        ["layouts", file] => file.starts_with("cswap."),
        ["mods", dir, _file] => dir.starts_with("cswap-"),
        _ => false,
    };
    plain && inside && ours && parts.iter().all(|p| !p.is_empty() && !p.starts_with('.'))
}

/// Where the files come from.
enum Source {
    /// A checkout of this repository: `CSWAP_INTEGRATIONS=<repo>/integrations`.
    Dir(PathBuf),
    /// The repository on GitHub, at one tag or branch.
    Web(String),
}

impl Source {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        match self {
            Source::Dir(dir) => {
                let file = dir.join(path);
                fs::read(&file).with_context(|| format!("cannot read {}", file.display()))
            }
            Source::Web(base) => download(&format!("{base}/{path}")),
        }
    }
}

fn download(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .set("User-Agent", http::USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .with_context(|| format!("cannot download {url}"))?;
    let mut buf = Vec::new();
    // A cap, so a wrong URL cannot fill the disk.
    resp.into_reader()
        .take(4 * 1024 * 1024)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

/// The source and its list of files. On GitHub the files of this version's
/// tag come first, because they match this binary; a build that is ahead of
/// its release falls back to the main branch.
fn open() -> Result<(Source, Manifest)> {
    let parse = |bytes: &[u8]| -> Result<Manifest> {
        serde_json::from_slice(bytes).context("the herdr integration's manifest.json is not valid")
    };
    if let Some(dir) = std::env::var_os("CSWAP_INTEGRATIONS").filter(|v| !v.is_empty()) {
        let src = Source::Dir(PathBuf::from(dir).join("herdr"));
        let manifest = parse(&src.read("manifest.json")?)?;
        return Ok((src, manifest));
    }
    if http::offline() {
        bail!("cswap is offline (CSWAP_OFFLINE is set), so it cannot fetch the herdr files");
    }
    let mut last = None;
    for at in [format!("v{}", update::CURRENT), "main".to_string()] {
        let src = Source::Web(format!(
            "https://raw.githubusercontent.com/{}/{at}/integrations/herdr",
            update::REPO
        ));
        match src.read("manifest.json") {
            Ok(bytes) => return Ok((src, parse(&bytes)?)),
            Err(e) => last = Some(e),
        }
    }
    Err(last.expect("two sources were tried")).context(format!(
        "the herdr files for cswap {} could not be fetched from GitHub",
        update::CURRENT
    ))
}

/// Write beside the target and rename over it: herdr reloads a file when it
/// changes, and must never read half of one.
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = parent.join(format!(".{name}.cswap-tmp.{}", std::process::id()));
    fs::write(&tmp, bytes).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// Move a file out of the way under a name nothing else has: `<file>.mine`,
/// then `<file>.mine.2`, and so on.
fn set_aside(path: &Path) -> Result<PathBuf> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let aside = (1..1000)
        .map(|n| match n {
            1 => path.with_file_name(format!("{name}.mine")),
            n => path.with_file_name(format!("{name}.mine.{n}")),
        })
        .find(|p| !p.exists())
        .context("too many files set aside already")?;
    fs::rename(path, &aside).with_context(|| format!("cannot move {} aside", path.display()))?;
    Ok(aside)
}

/// What an install did.
#[derive(Debug, Serialize)]
pub struct Installed {
    pub version: String,
    pub home: PathBuf,
    /// Files written, new or replaced.
    pub written: usize,
    /// Files that were there already, the same.
    pub unchanged: usize,
    /// Files of the user's own that were in the way, by their new names.
    pub set_aside: Vec<String>,
    /// Files an older install wrote that this one no longer has.
    pub removed: usize,
}

/// Fetch the herdr files and put them into herdr's folder.
pub fn install() -> Result<Installed> {
    let home = herdr_home();
    if !home.is_dir() {
        bail!(
            "herdr's folder {} is not there. Start herdr once, then install again",
            paths::tilde(&home)
        );
    }
    let (src, manifest) = open()?;
    let mut fetched = Vec::new();
    for path in &manifest.files {
        if !allowed(path) {
            bail!("the herdr integration lists {path}, which is not a file of the cswap layout. Nothing was changed");
        }
        fetched.push((path.as_str(), src.read(path)?));
    }
    if fetched.is_empty() {
        bail!("the herdr integration lists no files. Nothing was changed");
    }

    let old: Option<Record> = fsx::read_json(&record_path()).ok().flatten();
    let wrote = |path: &str| old.as_ref().and_then(|r| r.files.get(path));
    let mut done = Installed {
        version: manifest.version.clone(),
        home: home.clone(),
        written: 0,
        unchanged: 0,
        set_aside: Vec::new(),
        removed: 0,
    };
    let mut files = BTreeMap::new();
    for (path, bytes) in &fetched {
        let target = home.join(path);
        let new = hash(bytes);
        match fs::read(&target) {
            Ok(have) if hash(&have) == new => done.unchanged += 1,
            Ok(have) => {
                // The user's own, unless it is exactly what cswap put there.
                if wrote(path) != Some(&hash(&have)) {
                    let aside = set_aside(&target)?;
                    let name = aside.strip_prefix(&home).unwrap_or(&aside);
                    done.set_aside.push(name.display().to_string());
                }
                write(&target, bytes)?;
                done.written += 1;
            }
            Err(_) => {
                write(&target, bytes)?;
                done.written += 1;
            }
        }
        files.insert(path.to_string(), new);
    }
    // A file an older install wrote and this one dropped goes, if untouched.
    for (path, was) in old.iter().flat_map(|r| &r.files) {
        let target = home.join(path);
        let untouched = fs::read(&target).is_ok_and(|have| &hash(&have) == was);
        if !files.contains_key(path) && allowed(path) && untouched {
            let _ = fs::remove_file(&target);
            done.removed += 1;
        }
    }
    fsx::write_json(
        &record_path(),
        &Record {
            version: manifest.version,
            cswap: update::CURRENT.to_string(),
            installed_at: Utc::now(),
            files,
        },
    )?;
    Ok(done)
}

/// Where the herdr integration stands, from the disk alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// herdr's folder is not on this machine.
    NoHerdr,
    NotInstalled,
    Installed {
        record: Record,
        /// Files that are gone.
        missing: usize,
        /// Files edited since the install.
        changed: usize,
    },
}

impl State {
    /// Installed by another cswap than this one: its files may be older.
    pub fn stale(&self) -> bool {
        matches!(self, State::Installed { record, .. } if record.cswap != update::CURRENT)
    }

    /// One line for a list.
    pub fn line(&self) -> String {
        match self {
            State::NoHerdr => "herdr is not on this machine".to_string(),
            State::NotInstalled => "not installed".to_string(),
            State::Installed {
                missing, changed, ..
            } if *missing > 0 => {
                let _ = changed;
                format!("installed · {missing} files missing")
            }
            State::Installed { record, .. } if self.stale() => {
                format!("installed by cswap {} · install again", record.cswap)
            }
            State::Installed {
                record, changed, ..
            } if *changed > 0 => format!("installed {} · {changed} edited by you", record.version),
            State::Installed { record, .. } => format!("installed {}", record.version),
        }
    }

    /// Is there something an install would fix?
    pub fn wants_install(&self) -> bool {
        match self {
            State::NoHerdr => false,
            State::NotInstalled => true,
            State::Installed { missing, .. } => *missing > 0 || self.stale(),
        }
    }
}

pub fn state() -> State {
    let home = herdr_home();
    if !home.is_dir() {
        return State::NoHerdr;
    }
    let Some(record) = fsx::read_json::<Record>(&record_path()).ok().flatten() else {
        return State::NotInstalled;
    };
    let (mut missing, mut changed) = (0, 0);
    for (path, was) in &record.files {
        match fs::read(home.join(path)) {
            Ok(have) if &hash(&have) == was => {}
            Ok(_) => changed += 1,
            Err(_) => missing += 1,
        }
    }
    State::Installed {
        record,
        missing,
        changed,
    }
}

/// Ask the running herdr to show the cswap layout.
pub fn show() -> Result<()> {
    if !herdr_home()
        .join("layouts")
        .join(format!("{LAYOUT}.json"))
        .is_file()
    {
        bail!("the cswap layout is not installed. Install it first");
    }
    let bin = std::env::var("CSWAP_HERDR_APP").unwrap_or_else(|_| "herdr-app".to_string());
    let out = Command::new(&bin)
        .args(["do", "layout.switch", "--name", LAYOUT])
        .output()
        .with_context(|| {
            format!("cannot run {bin}. In herdr, pick \"{LAYOUT}\" under Settings → Layout")
        })?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        bail!(
            "herdr did not show the layout: {}. Is herdr running?",
            said.trim()
        );
    }
    Ok(())
}

/// `cswap herdr install`.
pub fn install_cmd(show_it: bool, as_json: bool) -> Result<()> {
    let done = install()?;
    if show_it {
        show()?;
    }
    if as_json {
        println!("{}", serde_json::to_string_pretty(&done)?);
        return Ok(());
    }
    let home = paths::tilde(&done.home);
    if done.written == 0 && done.removed == 0 {
        println!(
            "The cswap layout {} is already in {home}: {} files, all current.",
            done.version, done.unchanged
        );
    } else {
        println!(
            "Installed the cswap layout {} into {home}: {} files written, {} already current.",
            done.version, done.written, done.unchanged
        );
    }
    for name in &done.set_aside {
        println!("  Your own version of a file was kept as {name}");
    }
    if show_it {
        println!("herdr shows it now.");
    } else {
        println!(
            "Show it with: cswap herdr install --show   (or pick \"{LAYOUT}\" under Settings → Layout in herdr)"
        );
    }
    Ok(())
}

/// `cswap herdr status`. False when an install is wanted, so a script can ask.
pub fn status_cmd(as_json: bool) -> Result<bool> {
    let s = state();
    if as_json {
        let (version, by, missing, changed) = match &s {
            State::Installed {
                record,
                missing,
                changed,
            } => (
                Some(record.version.clone()),
                Some(record.cswap.clone()),
                *missing,
                *changed,
            ),
            _ => (None, None, 0, 0),
        };
        let v = serde_json::json!({
            "herdr": s != State::NoHerdr,
            "home": herdr_home(),
            "installed": matches!(s, State::Installed { .. }),
            "version": version,
            "installed_by": by,
            "missing": missing,
            "changed": changed,
            "wants_install": s.wants_install(),
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!("herdr: {}", s.line());
        if s.wants_install() {
            println!("Install it with: cswap herdr install");
        }
    }
    Ok(!s.wants_install())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Sandbox;

    fn repo() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("integrations")
    }

    #[test]
    fn only_the_layouts_own_files_may_be_written() {
        for ok in [
            "layouts/cswap.json",
            "layouts/cswap.svg",
            "mods/cswap-agents/main.js",
            "mods/cswap-chrome/cswap.svg",
        ] {
            assert!(allowed(ok), "{ok}");
        }
        for bad in [
            "layouts/main.json",
            "mods/agents/main.js",
            "mods/cswap-agents/../agents/main.js",
            "../layouts/cswap.json",
            "/etc/passwd",
            "keys.json",
            "mods/cswap-agents/.hidden",
            "mods/cswap-agents/a/b.js",
            "layouts/cswap.json\\..\\x",
            "mods/cswap-agents//main.js",
        ] {
            assert!(!allowed(bad), "{bad}");
        }
    }

    /// The list in the manifest is written by hand, so it is checked against
    /// what is in the folder: a file left out would never be installed.
    #[test]
    fn the_manifest_lists_every_file_of_the_integration() {
        let dir = repo().join("herdr");
        let manifest: Manifest =
            serde_json::from_slice(&fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        let mut on_disk = Vec::new();
        for top in ["layouts", "mods"] {
            let mut stack = vec![dir.join(top)];
            while let Some(d) = stack.pop() {
                for e in fs::read_dir(&d).unwrap().flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else {
                        let rel = p.strip_prefix(&dir).unwrap();
                        on_disk.push(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
        on_disk.sort();
        let mut listed = manifest.files.clone();
        listed.sort();
        assert_eq!(listed, on_disk);
        assert!(listed.iter().all(|p| allowed(p)));
        assert!(listed.contains(&format!("layouts/{LAYOUT}.json")));
    }

    #[test]
    fn an_install_writes_the_files_and_never_overwrites_the_users_own() {
        let _sb = Sandbox::new();
        let home = paths::home().join(".config").join("herdr-app");
        std::env::set_var("CSWAP_INTEGRATIONS", repo());
        std::env::remove_var("HERDR_APP_HOME");

        assert_eq!(state(), State::NoHerdr);
        assert!(install().unwrap_err().to_string().contains("Start herdr"));

        fs::create_dir_all(&home).unwrap();
        assert_eq!(state(), State::NotInstalled);
        let first = install().unwrap();
        assert!(first.written > 5);
        assert_eq!((first.unchanged, first.removed), (0, 0));
        assert!(first.set_aside.is_empty());
        assert!(home.join("layouts/cswap.json").is_file());
        assert!(home.join("mods/cswap-agents/main.js").is_file());
        assert!(matches!(
            state(),
            State::Installed {
                missing: 0,
                changed: 0,
                ..
            }
        ));
        assert!(!state().wants_install());

        // Again: nothing to do.
        let again = install().unwrap();
        assert_eq!((again.written, again.unchanged), (0, first.written));

        // A file the user edited is set aside, not lost.
        let mine = home.join("mods/cswap-agents/main.js");
        fs::write(&mine, "// mine").unwrap();
        assert!(state().line().contains("1 edited by you"));
        let third = install().unwrap();
        assert_eq!(third.written, 1);
        assert_eq!(third.set_aside, vec!["mods/cswap-agents/main.js.mine"]);
        assert_eq!(
            fs::read_to_string(home.join("mods/cswap-agents/main.js.mine")).unwrap(),
            "// mine"
        );
        assert_ne!(fs::read_to_string(&mine).unwrap(), "// mine");

        // A file that went missing is said, and an install brings it back.
        fs::remove_file(home.join("layouts/cswap.svg")).unwrap();
        assert!(state().wants_install());
        assert!(state().line().contains("1 files missing"));
        assert_eq!(install().unwrap().written, 1);

        std::env::remove_var("CSWAP_INTEGRATIONS");
    }
}
