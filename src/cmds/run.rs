//! `cswap run [NAME] [CLAUDE_ARGS]...` and the hidden `_claude` shim.
//!
//! Resolves the account (explicit name > $CSWAP_ACTIVE > default), makes the
//! profile launch-ready, then **exec**s the real claude binary with
//! CLAUDE_CONFIG_DIR pointing at the profile — zero wrapper overhead, native
//! signal handling and exit codes.
//!
//! Live passthrough: the account whose email matches the identity currently
//! logged into ~/.claude runs against ~/.claude itself (no CLAUDE_CONFIG_DIR,
//! no token handling by cswap). One credential copy means cswap can never
//! rotate the refresh-token family out from under the live login the VS Code
//! extension and bare `command claude` depend on.

use anyhow::{bail, Context, Result};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use crate::config::Config;
use crate::profile;

/// Env vars that make claude bypass account OAuth entirely — scrubbed so a
/// stray API key in the shell can't silently hijack a session's identity.
pub const SCRUBBED: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
];

/// `cswap run`: first arg is an account (alias|email) if it matches one,
/// otherwise every arg passes to claude and the active/default account is
/// used. With no args at all on a terminal, an interactive picker asks.
pub fn run(mut args: Vec<String>) -> Result<()> {
    let cfg = Config::load()?;
    let acct = match args.first() {
        Some(first) if cfg.find(first).is_some() => {
            let acct = cfg.find(first).expect("just checked").clone();
            args.remove(0);
            acct
        }
        None if crate::interactive::on_tty() && !cfg.accounts.is_empty() => {
            crate::interactive::pick_account(&cfg, "Run claude as which account?", &[])?
                .expect("no extra items")
                .clone()
        }
        _ => cfg.resolve_active()?,
    };
    launch(&acct, &args, true)
}

/// Hidden `_claude` shim used by the shell-init `claude()` wrapper: args pass
/// through verbatim (never interpreted as an account name). Quiet — a bare
/// `claude` should feel exactly like claude.
pub fn shim(args: Vec<String>) -> Result<()> {
    let cfg = Config::load()?;
    let acct = cfg.resolve_active()?;
    launch(&acct, &args, false)
}

fn launch(acct: &crate::config::Account, args: &[String], announce: bool) -> Result<()> {
    let config_dir = if profile::live_email().as_deref() == Some(acct.email.as_str()) {
        if announce {
            eprintln!(
                "cswap: running claude as {} ({}) [live ~/.claude]",
                acct.label(),
                acct.email
            );
        }
        None
    } else {
        let dir = profile::ensure(acct)?;
        if announce {
            eprintln!("cswap: running claude as {} ({})", acct.label(), acct.email);
        }
        Some(dir)
    };
    exec_claude(config_dir, args)
}

fn exec_claude(config_dir: Option<PathBuf>, args: &[String]) -> Result<()> {
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
        // Live passthrough: make sure a preset CLAUDE_CONFIG_DIR from the
        // environment can't redirect what "the live account" means.
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
