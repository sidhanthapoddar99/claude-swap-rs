//! `cswap run [NAME] [CLAUDE_ARGS]...` and the hidden `_claude` shim.
//!
//! Resolves the target (explicit name > $CSWAP_ACTIVE > `default`), makes the
//! profile launch-ready, then **exec**s the real claude binary with
//! CLAUDE_CONFIG_DIR pointing at the profile — zero wrapper overhead, native
//! signal handling and exit codes.
//!
//! `default` runs against ~/.claude itself: no CLAUDE_CONFIG_DIR, no profile,
//! no token handling by cswap. It is reached by selecting it, never by matching
//! emails — a profile that happens to hold the same account as the live login
//! still runs from its own directory with its own tokens.

use anyhow::{bail, Context, Result};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use crate::config::{Config, Target};
use crate::profile;

/// Env vars that make claude bypass account OAuth entirely — scrubbed so a
/// stray API key in the shell can't silently hijack a session's identity.
pub const SCRUBBED: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
];

/// `cswap run`: the first arg is a target (name|alias|`default`) if it matches
/// one, otherwise every arg passes to claude and the active target is used.
/// With no args at all on a terminal, an interactive picker asks.
pub fn run(mut args: Vec<String>) -> Result<()> {
    let cfg = Config::load()?;
    let target = match args.first() {
        Some(first) if is_target(&cfg, first) => {
            let target = cfg.resolve_key(first)?;
            args.remove(0);
            target
        }
        None if crate::interactive::on_tty() => {
            crate::interactive::pick_target(&cfg, "Run claude as which profile?")?
        }
        _ => cfg.resolve_active()?,
    };
    launch(&target, &args, true)
}

/// Hidden `_claude` shim used by the shell-init `claude()` wrapper: args pass
/// through verbatim (never interpreted as a target). Quiet — a bare `claude`
/// should feel exactly like claude.
pub fn shim(args: Vec<String>) -> Result<()> {
    let cfg = Config::load()?;
    let target = cfg.resolve_active()?;
    launch(&target, &args, false)
}

fn is_target(cfg: &Config, key: &str) -> bool {
    crate::config::is_default_key(key) || cfg.find(key).is_some()
}

fn launch(target: &Target, args: &[String], announce: bool) -> Result<()> {
    let config_dir = match target {
        Target::Default => {
            if announce {
                let who = profile::live_email().unwrap_or_else(|| "nobody logged in".to_string());
                eprintln!("cswap: running claude as default ({who}) [live ~/.claude]");
            }
            None
        }
        Target::Profile(p) => {
            let dir = profile::ensure(p)?;
            if announce {
                eprintln!("cswap: running claude as {} ({})", p.label(), p.email);
            }
            Some(dir)
        }
    };
    exec_claude(config_dir, args)
}

pub fn exec_claude(config_dir: Option<PathBuf>, args: &[String]) -> Result<()> {
    let claude = find_claude()?;
    let mut cmd = Command::new(&claude);
    cmd.args(args);
    for var in SCRUBBED {
        cmd.env_remove(var);
    }
    match &config_dir {
        Some(dir) => {
            cmd.env("CLAUDE_CONFIG_DIR", dir);
        }
        // default: make sure a preset CLAUDE_CONFIG_DIR from the environment
        // can't redirect what "the live ~/.claude" means.
        None => {
            cmd.env_remove("CLAUDE_CONFIG_DIR");
        }
    }
    // exec() only returns on failure.
    let err = cmd.exec();
    Err(err).with_context(|| format!("failed to exec {}", claude.display()))
}

pub fn find_claude() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("CSWAP_CLAUDE_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
        bail!("CSWAP_CLAUDE_BIN is set but not a file: {}", p.display());
    }
    let path = std::env::var_os("PATH").context("PATH is not set")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("claude");
        if candidate.is_file() && is_executable(&candidate) {
            return Ok(candidate);
        }
    }
    bail!("`claude` not found on PATH — install Claude Code first (or set CSWAP_CLAUDE_BIN)");
}

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}
