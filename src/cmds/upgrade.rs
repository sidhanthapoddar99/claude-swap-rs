//! `cswap upgrade` — self-update from GitHub Releases.
//!
//! Downloads the latest release asset for this platform, verifies its SHA-256
//! against the published checksum, and atomically replaces the running binary
//! (write beside it + rename — safe on Unix even while executing).

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::process::Command;

use crate::update_check::{fetch_latest_version, version_newer, REPO};

/// The release-asset target triple this binary was built for.
fn target() -> Result<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("x86_64-unknown-linux-musl")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Ok("aarch64-unknown-linux-musl")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Ok("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok("aarch64-apple-darwin")
    } else {
        bail!("no prebuilt binaries for this platform — upgrade with cargo install")
    }
}

pub fn run() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let latest = fetch_latest_version()
        .context("could not reach the GitHub releases API — check your network")?;
    if !version_newer(&latest, current) {
        println!("cswap v{current} is already the latest release.");
        return Ok(());
    }
    println!("Upgrading v{current} → v{latest}...");

    let target = target()?;
    let asset = format!("cswap-{target}.tar.gz");
    let base = format!("https://github.com/{REPO}/releases/download/v{latest}");

    let tarball = download(&format!("{base}/{asset}"))?;
    let checksum_line = String::from_utf8_lossy(&download(&format!("{base}/{asset}.sha256"))?)
        .trim()
        .to_string();
    let expected = checksum_line
        .split_whitespace()
        .next()
        .context("empty checksum file")?
        .to_lowercase();
    let actual = hex(&Sha256::digest(&tarball));
    if actual != expected {
        bail!("checksum mismatch (expected {expected}, got {actual}) — aborting");
    }

    // Unpack next to the running binary so the final rename stays on one
    // filesystem (atomic replace).
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("cannot locate the running cswap binary")?;
    let dir = exe.parent().context("binary has no parent directory")?;
    let work = dir.join(".cswap-upgrade");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).with_context(|| {
        format!(
            "cannot write to {} — rerun with appropriate permissions",
            dir.display()
        )
    })?;
    let tar_path = work.join(&asset);
    fs::write(&tar_path, &tarball)?;
    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(&tar_path)
        .arg("-C")
        .arg(&work)
        .status()
        .context("failed to run tar")?;
    if !status.success() {
        bail!("tar extraction failed");
    }
    let new_bin = work.join("cswap");
    fs::set_permissions(&new_bin, {
        use std::os::unix::fs::PermissionsExt;
        fs::Permissions::from_mode(0o755)
    })?;
    fs::rename(&new_bin, &exe).context("failed to replace the binary")?;
    let _ = fs::remove_dir_all(&work);

    println!("Done: {} is now v{latest}.", exe.display());
    Ok(())
}

fn download(url: &str) -> Result<Vec<u8>> {
    let resp = ureq::get(url)
        .set("User-Agent", crate::oauth::USER_AGENT)
        .call()
        .with_context(|| format!("download failed: {url}"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(64 * 1024 * 1024) // sanity cap
        .read_to_end(&mut buf)?;
    Ok(buf)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn target_resolves_on_supported_platforms() {
        // On any platform we ship releases for, this must be Ok.
        assert!(super::target().is_ok());
    }

    #[test]
    fn hex_encodes() {
        assert_eq!(super::hex(&[0xde, 0xad, 0x01]), "dead01");
    }
}
