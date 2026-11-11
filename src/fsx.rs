//! Small filesystem helpers: private atomic writes, JSON files, atomic links
//! and an advisory lock.

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;

use crate::platform;
use std::path::{Path, PathBuf};

/// Create a directory (and its parents) readable by the owner only.
pub fn private_dir(path: &Path) -> Result<()> {
    // Every directory this call creates is made private, not only the last:
    // the first write into `~/.cswap/a/b` must not leave `~/.cswap` open.
    let missing: Vec<&Path> = path.ancestors().take_while(|p| !p.exists()).collect();
    fs::create_dir_all(path).with_context(|| format!("cannot create {}", path.display()))?;
    for dir in missing {
        platform::make_private(dir, true)?;
    }
    platform::make_private(path, true)?;
    Ok(())
}

/// Write `contents` to `path` with mode 0600, by writing a sibling and
/// renaming it over the target. A reader never sees half a file.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    if !parent.exists() {
        private_dir(parent)?;
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = parent.join(format!(".{name}.cswap-tmp.{}", std::process::id()));
    {
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        let mut f = platform::private_open(&mut opts)
            .open(&tmp)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        f.write_all(contents)?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(())
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    write_private(path, text.as_bytes())
}

/// Read a JSON file into `T`. A missing file is `Ok(None)`; a file that does
/// not parse is an error, because overwriting it with defaults would lose it.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    serde_json::from_str(&text)
        .map(Some)
        .with_context(|| format!("{} is not valid JSON for cswap", path.display()))
}

/// Read any JSON file as a loose value; `None` when missing or unparseable.
pub fn read_value(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Point `link` at `target` without ever leaving the path absent.
///
/// Removing then linking opens a window in which a running agent finds nothing
/// at the path and creates a real directory there, which then hides the shared
/// one for good. A rename over the old link has no such window.
pub fn link_atomic(target: &Path, link: &Path) -> Result<()> {
    let name = link.file_name().unwrap_or_default().to_string_lossy();
    let tmp = link.with_file_name(format!(".cswap-link.{name}"));
    let _ = platform::remove_link(&tmp);
    platform::symlink(target, &tmp)
        .with_context(|| format!("cannot stage a link at {}", tmp.display()))?;
    let mut moved = fs::rename(&tmp, link);
    if moved.is_err() && cfg!(windows) {
        // Windows cannot rename over a directory link: take the old one away
        // first. This is the one place the path is briefly absent.
        let _ = platform::remove_link(link);
        moved = fs::rename(&tmp, link);
    }
    moved.inspect_err(|_| {
        let _ = platform::remove_link(&tmp);
    })?;
    Ok(())
}

/// Hold an exclusive advisory lock on `<dir>/.lock` while `f` runs, so two
/// cswap processes never interleave a read-modify-write of the same file.
pub fn with_lock<T>(dir: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    if !dir.exists() {
        private_dir(dir)?;
    }
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(false);
    let lock: File = platform::private_open(&mut opts).open(dir.join(".lock"))?;
    lock.lock()?;
    let out = f();
    let _ = lock.unlock();
    out
}

/// Find `name` on `PATH`, skipping `skip` (cswap itself, when it is installed
/// under an agent's name).
pub fn which(name: &str, skip: Option<&Path>) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names = platform::program_names(name);
    for dir in std::env::split_paths(&path) {
        for file in &names {
            let candidate = dir.join(file);
            let Ok(md) = fs::metadata(&candidate) else {
                continue;
            };
            if !platform::is_program(&md) {
                continue;
            }
            if let Some(skip) = skip {
                if fs::canonicalize(&candidate).ok().as_deref() == Some(skip) {
                    continue;
                }
            }
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn private_write_is_0600_and_replaces() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b.json");
        write_private(&p, b"one").unwrap();
        write_private(&p, b"two").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "two");
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // No temp file is left behind.
        assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn every_directory_a_write_creates_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write_private(&dir.path().join("root/harness/claude/settings.json"), b"{}").unwrap();
        for sub in ["root", "root/harness", "root/harness/claude"] {
            let mode = fs::metadata(dir.path().join(sub))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700, "{sub}");
        }
    }

    #[test]
    fn link_atomic_retargets_an_existing_link() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, l) = (
            dir.path().join("a"),
            dir.path().join("b"),
            dir.path().join("l"),
        );
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        link_atomic(&a, &l).unwrap();
        link_atomic(&b, &l).unwrap();
        assert_eq!(fs::read_link(&l).unwrap(), b);
    }

    #[test]
    fn missing_json_is_none_and_bad_json_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.json");
        assert!(read_json::<Value>(&p).unwrap().is_none());
        fs::write(&p, "{nope").unwrap();
        assert!(read_json::<Value>(&p).is_err());
    }
}
