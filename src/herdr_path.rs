//! herdr's core keeps the `PATH` it started with. When that one does not have
//! cswap's shims first, `hc run claude …` starts the real program and the
//! agent lands on the root login. herdr can take a new environment without
//! ending a program (`hc server restart --fresh`), so cswap gives it its own
//! old one back with the shims folder put first, and nothing else changed.
//!
//! The cswap layout asks for that by itself (`cswap herdr path --auto`), so a
//! reboot does not leave herdr skipping cswap until someone notices.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::harness::Harness;
use crate::{fsx, paths, shim, store};

/// Do the agents herdr starts by a bare name go through cswap?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// No PATH shim is on, so there is nothing for herdr to find.
    Off,
    /// herdr does not answer: not installed, or not running.
    NoHerdr,
    /// Every agent with a shim is started through cswap.
    Fine,
    /// These are not: each with the program herdr starts instead, if any.
    Skips(Vec<(Harness, Option<PathBuf>)>),
}

/// The agents whose shim is on and whole.
fn agents() -> Vec<Harness> {
    let settings = store::Settings::load().unwrap_or_default();
    shim::chosen(&settings)
        .into_iter()
        .filter(|h| shim::state(*h) != shim::State::Broken)
        .collect()
}

pub fn state() -> State {
    let agents = agents();
    if agents.is_empty() {
        return State::Off;
    }
    let Some(seen) = shim::seen_by_herdr(&agents) else {
        return State::NoHerdr;
    };
    let skips: Vec<_> = seen
        .into_iter()
        .filter(|(_, _, ours)| !ours)
        .map(|(h, first, _)| (h, first))
        .collect();
    if skips.is_empty() {
        State::Fine
    } else {
        State::Skips(skips)
    }
}

/// `old` with `dir` put first, and taken out of any later place.
fn with_first(old: Option<&OsStr>, dir: &Path) -> Result<OsString> {
    let rest = old
        .map(|o| std::env::split_paths(o).collect::<Vec<_>>())
        .unwrap_or_default();
    let all = std::iter::once(dir.to_path_buf()).chain(rest.into_iter().filter(|p| p != dir));
    std::env::join_paths(all).context("herdr's PATH has a folder that cannot be written back")
}

/// The environment a running process was started with.
#[cfg(unix)]
fn environ_of(pid: u64) -> Result<Vec<(OsString, OsString)>> {
    use std::os::unix::ffi::OsStrExt;
    let file = std::env::var_os("CSWAP_PROC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/proc"))
        .join(pid.to_string())
        .join("environ");
    let raw = std::fs::read(&file).with_context(|| {
        format!(
            "cannot read the environment of herdr's core ({})",
            file.display()
        )
    })?;
    let env: Vec<_> = raw
        .split(|b| *b == 0)
        .filter_map(|pair| {
            let at = pair.iter().position(|b| *b == b'=')?;
            (at > 0).then(|| {
                (
                    OsStr::from_bytes(&pair[..at]).to_os_string(),
                    OsStr::from_bytes(&pair[at + 1..]).to_os_string(),
                )
            })
        })
        .collect();
    if env.is_empty() {
        bail!("the environment of herdr's core reads as empty");
    }
    Ok(env)
}

#[cfg(not(unix))]
fn environ_of(_pid: u64) -> Result<Vec<(OsString, OsString)>> {
    bail!("the environment of herdr's core cannot be read on this system")
}

/// What to do by hand when cswap cannot do it.
const BY_HAND: &str = "Run `hc server restart --fresh` in a terminal outside herdr instead";

/// What herdr still started after a restart that should have put it right,
/// one `bin=path` each. The automatic fix does not restart herdr for the
/// same again: it would do so at every check and never help.
fn failed_file() -> PathBuf {
    paths::root().join("herdr-path-failed.json")
}

fn told(skips: &[(Harness, Option<PathBuf>)]) -> Vec<String> {
    skips
        .iter()
        .map(|(h, first)| {
            format!(
                "{}={}",
                h.spec().bins[0],
                first
                    .as_deref()
                    .map_or(String::new(), |p| p.display().to_string())
            )
        })
        .collect()
}

/// Did a restart leave herdr exactly like this before?
fn failed_before(skips: &[(Harness, Option<PathBuf>)]) -> bool {
    fsx::read_json::<Vec<String>>(&failed_file())
        .ok()
        .flatten()
        .is_some_and(|was| was == told(skips))
}

/// Give herdr's core its own environment back with the shims folder first.
fn restart() -> Result<()> {
    let status = crate::herdr::hc(&["server", "status", "--json"])?;
    let pid = serde_json::from_str::<serde_json::Value>(&status)
        .ok()
        .and_then(|v| v.get("pid").and_then(serde_json::Value::as_u64))
        .with_context(|| format!("herdr did not say which process its core is. {BY_HAND}."))?;
    // The core's own environment, so that only the PATH changes: this
    // process's one may carry an agent's account and herdr's terminal.
    let mut env = environ_of(pid).with_context(|| format!("{BY_HAND}."))?;
    let old = env
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.clone());
    let path = with_first(old.as_deref(), &shim::dir())?;
    env.retain(|(k, _)| k != "PATH" && k != "HC_TERMINAL_ID");

    // Found on this PATH, before the environment is replaced.
    let bin = match std::env::var_os("CSWAP_HC") {
        Some(bin) => PathBuf::from(bin),
        None => crate::fsx::which("hc", None).context("herdr's `hc` is not on PATH")?,
    };
    let out = Command::new(&bin)
        .args(["server", "restart", "--fresh", "--json"])
        .env_clear()
        .envs(env)
        .env("PATH", &path)
        .output()
        .context("herdr's `hc` could not be run")?;
    if !out.status.success() {
        bail!(
            "herdr did not restart its core: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    shim::forget_herdr();
    Ok(())
}

/// Restart herdr's core with the shims folder first on its `PATH`. Running
/// programs keep running; terminals started from now on find cswap.
pub fn fix() -> Result<State> {
    match state() {
        State::Skips(_) => {}
        other => return Ok(other),
    }
    restart()?;
    match state() {
        State::Skips(left) => {
            let _ = fsx::write_json(&failed_file(), &told(&left));
            let (h, first) = &left[0];
            bail!(
                "herdr restarted its core and still starts {} for a bare {}",
                first.as_deref().map_or("nothing".to_string(), paths::tilde),
                h.spec().bins[0]
            )
        }
        State::NoHerdr => bail!("herdr restarted its core and does not answer yet"),
        fine => Ok(fine),
    }
}

/// How `cswap herdr path` is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// Only say where herdr stands.
    Look,
    /// Put it right.
    Fix,
    /// Put it right when the setting allows it, and not twice in vain: what
    /// the cswap layout asks, unseen, when it starts and every few minutes.
    Auto,
}

/// What became of one asking.
struct Outcome {
    now: State,
    /// herdr's core was restarted.
    fixed: bool,
    /// An automatic fix was held back: the last restart left herdr as it is.
    gave_up: bool,
}

fn answer(ask: Ask) -> Result<Outcome> {
    let before = state();
    let State::Skips(skips) = &before else {
        if before == State::Fine {
            let _ = std::fs::remove_file(failed_file());
        }
        return Ok(Outcome {
            now: before,
            fixed: false,
            gave_up: false,
        });
    };
    let gave_up = ask == Ask::Auto && failed_before(skips);
    let fixed = match ask {
        Ask::Look => false,
        Ask::Fix => true,
        Ask::Auto => !gave_up && store::Settings::load().unwrap_or_default().herdr_fix_path,
    };
    let now = if fixed { fix()? } else { before };
    if now == State::Fine {
        let _ = std::fs::remove_file(failed_file());
    }
    Ok(Outcome {
        now,
        fixed,
        gave_up,
    })
}

/// `cswap herdr path`: say where herdr stands, and with `Ask::Fix` or
/// `Ask::Auto` put it right. False when a bare name still skips cswap.
pub fn path_cmd(ask: Ask, json: bool) -> Result<bool> {
    // One at a time: two windows of the layout ask at the same moment, and
    // the second must find herdr already put right.
    let Outcome {
        now,
        fixed,
        gave_up,
    } = match ask {
        Ask::Look => answer(ask)?,
        _ => fsx::with_lock(&paths::root(), || answer(ask))?,
    };
    let skips: Vec<(Harness, Option<PathBuf>)> = match &now {
        State::Skips(s) => s.clone(),
        _ => Vec::new(),
    };
    if json {
        let word = match now {
            State::Off => "off",
            State::NoHerdr => "no_herdr",
            State::Fine => "fine",
            State::Skips(_) => "skips",
        };
        let rows: Vec<_> = skips
            .iter()
            .map(|(h, first)| {
                serde_json::json!({
                    "agent": h.id(),
                    "bin": h.spec().bins[0],
                    "starts": first.as_deref().map(|p| p.display().to_string()),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "state": word,
                "fixed": fixed,
                "gave_up": gave_up,
                "auto": store::Settings::load().unwrap_or_default().herdr_fix_path,
                "skips": rows,
            })
        );
        return Ok(skips.is_empty());
    }
    match now {
        State::Off => println!("PATH shims are off, so herdr starts the agents themselves. Turn them on with `cswap shims on`."),
        State::NoHerdr => println!("herdr does not answer: it is not installed or not running."),
        State::Fine if fixed => println!("herdr's core restarted with cswap's shims first on its PATH. Running agents kept running; agents started from now on go through cswap."),
        State::Fine => println!("herdr starts the agents through cswap."),
        State::Skips(_) => {
            for (h, first) in &skips {
                let bin = h.spec().bins[0];
                match first {
                    Some(first) => println!("herdr starts {} for a bare {bin}, not cswap's shim", paths::tilde(first)),
                    None => println!("herdr finds no {bin} at all"),
                }
            }
            if gave_up {
                println!("The last restart of herdr's core left it like this, so cswap does not restart it again by itself.");
            }
            println!("An agent started that way runs on the root login. Put it right with:\n  cswap herdr path --fix");
        }
    }
    Ok(skips.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shims_folder_goes_first_and_is_not_named_twice() {
        let dir = Path::new("/home/me/.cswap/shims");
        let path = |old: &str| {
            with_first(Some(OsStr::new(old)), dir)
                .unwrap()
                .into_string()
                .unwrap()
        };
        assert_eq!(path("/usr/bin:/bin"), "/home/me/.cswap/shims:/usr/bin:/bin");
        assert_eq!(
            path("/usr/bin:/home/me/.cswap/shims:/bin"),
            "/home/me/.cswap/shims:/usr/bin:/bin"
        );
        assert_eq!(
            with_first(None, dir).unwrap().into_string().unwrap(),
            "/home/me/.cswap/shims"
        );
    }
}
