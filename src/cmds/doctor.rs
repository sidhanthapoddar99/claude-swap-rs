//! `cswap doctor` — find the failures that are otherwise silent.
//!
//! One failure mode motivates this whole command. If a real file or directory
//! sits in a profile where a share link belongs, claude writes into it and the
//! data is invisible to `~/.claude` and to every other profile. Nothing errors.
//! The session looks fine until you go looking for a transcript that is not
//! there. `sync_links` refuses to clobber such a file — correctly, since the
//! files inside may be the only copy — so the only remaining job is to say so.
//!
//! `--repair` moves the shadowing entry into cswap's own space and restores the
//! link. It does NOT merge anything into ~/.claude: cswap never writes there,
//! and that rule does not get an exception for the recovery path. It prints
//! what it parked and leaves the merge to you.

use anyhow::{bail, Result};
use std::fs;
use std::path::Path;

use crate::config::Config;
use crate::ui::{self, ACCENT, BOLD, DIM};
use crate::{paths, profile};

pub fn run(repair: bool) -> Result<()> {
    let cfg = Config::load()?;
    let color = ui::color_on();
    let mut problems = 0;

    println!(
        "{} {}",
        ui::paint(color, DIM, "default —"),
        ui::paint(
            color,
            DIM,
            "the live ~/.claude, read-only to cswap. Nothing to check."
        )
    );

    if crate::config::needs_migration() {
        problems += 1;
        println!();
        println!(
            "{}  config.toml still uses a pre-0.6.1 shape. Run: cswap migrate",
            ui::paint(color, ACCENT, "stale config")
        );
    }

    if cfg.profiles.is_empty() {
        println!();
        println!(
            "{}",
            ui::paint(color, DIM, "No profiles yet. Run: cswap login <email>")
        );
        return Ok(());
    }

    for prof in &cfg.profiles {
        let dir = paths::profile_dir(&prof.email);
        println!();
        println!("{}", ui::paint(color, BOLD, &prof.email));

        if !dir.is_dir() {
            problems += 1;
            report(
                color,
                "missing",
                &format!("no directory at {}", dir.display()),
            );
            report(color, "fix", &format!("cswap login {}", prof.email));
            continue;
        }

        let sessions = profile::live_sessions(&dir);
        if !sessions.is_empty() {
            let pids: Vec<String> = sessions.iter().map(u32::to_string).collect();
            report(
                color,
                "in use",
                &format!("claude is running (pid {})", pids.join(", ")),
            );
        }

        if !dir.join(".credentials.json").exists() {
            problems += 1;
            report(color, "no login", "this profile has no credentials");
            report(color, "fix", &format!("cswap login {}", prof.email));
        }

        let broken = broken_links(&dir);
        if !broken.is_empty() {
            problems += 1;
            report(color, "dangling", &broken.join(", "));
            report(color, "fix", "cleared automatically on the next launch");
        }

        let shadows = profile::shadows(&dir);
        if shadows.is_empty() {
            if broken.is_empty() && dir.join(".credentials.json").exists() {
                report(color, "ok", "links healthy, login present");
            }
            continue;
        }

        problems += 1;
        for name in &shadows {
            let path = dir.join(name);
            report(
                color,
                "shadowed",
                &format!(
                    "{name}  ({}) — written here, invisible elsewhere",
                    describe(&path)
                ),
            );
        }

        if !repair {
            report(color, "fix", "cswap doctor --repair");
            continue;
        }
        if !sessions.is_empty() {
            bail!(
                "claude is running in {} (pid {}) — exit it before repairing.\n\
                 Moving a directory under a live session is what created this in \
                 the first place.",
                prof.email,
                sessions
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        repair_profile(&prof.email, &dir, &shadows, color)?;
    }

    println!();
    if problems == 0 {
        println!("{}", ui::paint(color, DIM, "No problems found."));
    } else if !repair {
        println!(
            "{}",
            ui::paint(
                color,
                DIM,
                &format!("{problems} problem(s). Re-run with --repair to fix what is fixable.")
            )
        );
    }
    Ok(())
}

/// Move each shadowing entry into cswap's own space, then restore the link.
///
/// The parked copy is the point: those files are what claude wrote while the
/// link was missing, and they may be the only copy in existence. Deleting them
/// to "fix" the link would repeat the original loss with extra steps.
fn repair_profile(email: &str, dir: &Path, shadows: &[String], color: bool) -> Result<()> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let park = paths::shadowed_dir(email).join(&stamp);
    fs::create_dir_all(&park)?;

    for name in shadows {
        let from = dir.join(name);
        let to = park.join(name);
        fs::rename(&from, &to)?;
        report(color, "parked", &format!("{name} -> {}", to.display()));
    }

    // Re-run the mirror now that nothing is in the way.
    let still = profile::sync_links(dir)?;
    if still.is_empty() {
        report(color, "relinked", "every share link restored");
    } else {
        report(
            color,
            "partial",
            &format!("still shadowed: {}", still.join(", ")),
        );
    }
    println!(
        "  {}",
        ui::paint(
            color,
            DIM,
            &format!(
                "Nothing was deleted and nothing was written into ~/.claude. Compare {} \
                 against ~/.claude and merge what you want to keep.",
                park.display()
            )
        )
    );
    Ok(())
}

/// Links pointing at a target that no longer exists.
fn broken_links(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_symlink()).unwrap_or(false) && !e.path().exists())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    out.sort();
    out
}

/// "3 files" / "empty directory" / "file" — enough to judge whether it matters.
fn describe(path: &Path) -> String {
    if !path.is_dir() {
        return "file".to_string();
    }
    let count = walk_count(path);
    match count {
        0 => "empty directory".to_string(),
        1 => "1 file".to_string(),
        n => format!("{n} files"),
    }
}

fn walk_count(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => walk_count(&e.path()),
            _ => 1,
        })
        .sum()
}

fn report(color: bool, tag: &str, text: &str) {
    let styled = match tag {
        "ok" | "parked" | "relinked" => ui::paint(color, DIM, tag),
        _ => ui::paint(color, ACCENT, tag),
    };
    println!("  {}  {text}", ui::pad(tag, &styled, 9));
}
