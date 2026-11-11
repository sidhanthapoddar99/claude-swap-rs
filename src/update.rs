//! Updates: a check against GitHub at most once every 24 hours, and
//! `cswap update`, which installs the latest release over the running binary.
//!
//! The check never runs for a wrapped launch (`cswap claude …`): starting an
//! agent touches no network. It runs for the plain commands and while the
//! dashboard is open, and it only ever tells; installing is `cswap update`.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::Command;

use crate::store::Settings;
use crate::{fsx, http, paths};

pub const REPO: &str = "sidhanthapoddar99/claude-swap-rs";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Serialize, Deserialize)]
struct Cache {
    checked_at: DateTime<Utc>,
    /// Empty when the check failed, so an offline machine does not retry on
    /// every command.
    latest: String,
}

fn cache_path() -> PathBuf {
    paths::cache_dir().join("update.json")
}

pub fn enabled(settings: &Settings) -> bool {
    settings.update_check && std::env::var_os("CSWAP_NO_UPDATE_CHECK").is_none()
}

/// True when `a` is a strictly newer x.y.z than `b`. Anything that is not a
/// number counts as 0, so an odd tag can never panic: at worst, no notice.
pub fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .map(|s| {
                s.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let (a, b) = (parse(a), parse(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// The latest release's version, without the leading `v`.
fn fetch_latest(timeout_secs: u64) -> Option<String> {
    if http::offline() {
        return None;
    }
    let resp: serde_json::Value = ureq::get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))
    .set("User-Agent", http::USER_AGENT)
    .set("Accept", "application/vnd.github+json")
    .timeout(std::time::Duration::from_secs(timeout_secs))
    .call()
    .ok()?
    .into_json()
    .ok()?;
    resp.get("tag_name")?
        .as_str()
        .map(|t| t.trim_start_matches('v').to_string())
}

fn read_cache() -> Option<Cache> {
    fsx::read_json(&cache_path()).ok().flatten()
}

/// A newer version, from what the last check stored. No network.
pub fn known_newer() -> Option<String> {
    read_cache().map(|c| c.latest).filter(|v| newer(v, CURRENT))
}

/// Ask GitHub when the last check is more than 24 hours old, and return a
/// newer version if there is one. Silent on every failure.
pub fn check_if_due() -> Option<String> {
    let fresh = read_cache().is_some_and(|c| Utc::now() - c.checked_at < Duration::hours(24));
    if !fresh {
        let latest = fetch_latest(3).unwrap_or_default();
        let _ = fsx::write_json(
            &cache_path(),
            &Cache {
                checked_at: Utc::now(),
                latest,
            },
        );
    }
    known_newer()
}

/// One dim line on stderr when a newer release exists. Only when a person is
/// watching, so scripts and pipes never trigger a request.
pub fn nudge(settings: &Settings) {
    if !enabled(settings) || !std::io::stderr().is_terminal() || !std::io::stdout().is_terminal() {
        return;
    }
    if let Some(latest) = check_if_due() {
        eprintln!(
            "\x1b[2mcswap {latest} is out (you have {CURRENT}). Install it with: cswap update\x1b[0m"
        );
    }
}

/// The release asset built for this machine, and whether it is a zip.
fn asset() -> Result<(String, bool)> {
    let target = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-musl"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-musl"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else {
        bail!(
            "there is no prebuilt cswap for this machine. Update with: cargo install --git https://github.com/{REPO}"
        )
    };
    let zip = cfg!(windows);
    Ok((
        format!("cswap-{target}.{}", if zip { "zip" } else { "tar.gz" }),
        zip,
    ))
}

fn download(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .set("User-Agent", http::USER_AGENT)
        .timeout(std::time::Duration::from_secs(120))
        .call()
        .with_context(|| format!("cannot download {url}"))?;
    let mut buf = Vec::new();
    // A cap, so a wrong URL cannot fill the disk.
    resp.into_reader()
        .take(64 * 1024 * 1024)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `cswap update`: install the latest release in place of this binary.
pub fn run(check_only: bool) -> Result<()> {
    let latest = fetch_latest(10).context("cannot reach GitHub to look for a release")?;
    let _ = fsx::write_json(
        &cache_path(),
        &Cache {
            checked_at: Utc::now(),
            latest: latest.clone(),
        },
    );
    if !newer(&latest, CURRENT) {
        println!("cswap {CURRENT} is the latest release.");
        return Ok(());
    }
    if check_only {
        println!("cswap {latest} is out (you have {CURRENT}). Install it with: cswap update");
        return Ok(());
    }
    println!("Updating cswap {CURRENT} → {latest}");

    let (asset, zip) = asset()?;
    let base = format!("https://github.com/{REPO}/releases/download/v{latest}");
    let archive = download(&format!("{base}/{asset}"))?;
    let sums = download(&format!("{base}/{asset}.sha256"))?;
    let expected = String::from_utf8_lossy(&sums)
        .split_whitespace()
        .next()
        .context("the release's checksum file is empty")?
        .to_lowercase();
    let actual = hex(&Sha256::digest(&archive));
    if actual != expected {
        bail!("the download does not match its checksum (expected {expected}, got {actual}). Nothing was changed");
    }

    // Unpack beside the running binary, so the final rename stays on one
    // filesystem and is atomic.
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("cannot find the running cswap binary")?;
    let dir = exe.parent().context("the binary has no parent directory")?;
    let work = dir.join(".cswap-update");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).with_context(|| {
        format!(
            "cannot write to {}. Run the update with the rights that installed cswap",
            dir.display()
        )
    })?;
    let result = (|| -> Result<()> {
        let file = work.join(&asset);
        fs::write(&file, &archive)?;
        // `tar` reads both formats; Windows has shipped one since 2018.
        let status = Command::new("tar")
            .arg(if zip { "-xf" } else { "-xzf" })
            .arg(&file)
            .arg("-C")
            .arg(&work)
            .status()
            .context("cannot run tar to unpack the release")?;
        if !status.success() {
            bail!("unpacking the release failed");
        }
        let name = if cfg!(windows) { "cswap.exe" } else { "cswap" };
        let fresh = work.join(name);
        if !fresh.is_file() {
            bail!("the release archive holds no {name}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fresh, fs::Permissions::from_mode(0o755))?;
        }
        if cfg!(windows) {
            // A running program cannot be overwritten on Windows, but it can
            // be renamed out of the way.
            let old = exe.with_extension("old.exe");
            let _ = fs::remove_file(&old);
            fs::rename(&exe, &old).context("cannot move the running binary aside")?;
        }
        fs::rename(&fresh, &exe).context("cannot put the new binary in place")?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&work);
    result?;
    println!("Done. {} is now cswap {latest}.", exe.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("1.0.0", "0.99.99"));
        assert!(newer("v0.7.1", "0.7.0"));
        assert!(!newer("0.7.0", "0.7.0"));
        assert!(!newer("0.6.3", "0.7.0"));
        assert!(!newer("garbage", "0.1.0"));
        assert!(!newer("", "0.1.0"));
        assert!(newer("0.8.0-beta", "0.7.9"));
    }

    #[test]
    fn there_is_a_release_asset_for_this_machine() {
        let (name, zip) = asset().unwrap();
        assert!(name.starts_with("cswap-"));
        assert_eq!(zip, cfg!(windows));
        assert_eq!(hex(&[0xde, 0xad, 0x01]), "dead01");
    }

    #[test]
    fn a_stored_check_answers_without_the_network() {
        let _sb = crate::testutil::Sandbox::new();
        assert_eq!(known_newer(), None);
        fsx::write_json(
            &cache_path(),
            &Cache {
                checked_at: Utc::now(),
                latest: "99.0.0".into(),
            },
        )
        .unwrap();
        assert_eq!(known_newer().as_deref(), Some("99.0.0"));
        // Fresh, so no request is made.
        assert_eq!(check_if_due().as_deref(), Some("99.0.0"));
    }
}
