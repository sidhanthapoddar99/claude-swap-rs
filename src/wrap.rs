//! The wrapper: `cswap claude …`, `cswap codex …`, and so on.
//!
//! Everything the user types goes to the agent untouched, except the flags
//! cswap keeps for itself and removes:
//!
//! - `--cswap` alone opens a picker. `--cswap-select` is the same.
//! - `--cswap=<account>` picks the account: a name, a number, an email,
//!   `root`, or `auto`. A comma-separated list is an order of preference:
//!   the first one that can start a session now is used.
//!
//! Without a flag the account is, in order: `$CSWAP_<HARNESS>` (set for
//! everything started inside a wrapped session, or by hand for one terminal),
//! then the harness's default from the settings.
//!
//! cswap then `exec`s the real binary, so signals, exit codes and the terminal
//! are the agent's own. Nothing here touches the network.

use anyhow::{bail, Context, Result};
use chrono::Utc;
use std::io::IsTerminal;
use std::process::Command;

use crate::harness::Harness;
use crate::store::{Account, HarnessSettings, Registry, Settings};
use crate::sync::State;
use crate::{manage, model, picker, platform, sessions, suggest};

/// cswap's own launch flags and what each does. The command-line help and
/// the dashboard's Help page both print this list.
pub const FLAGS: [(&str, &str); 4] = [
    ("--cswap", "pick the account from a list"),
    (
        "--cswap=<account>",
        "run on this account: a name, an alias, a number, root or auto",
    ),
    ("--cswap=a,b,c", "the first of these that is free right now"),
    ("--cswap-select", "the same as --cswap alone"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// One or more accounts, in order of preference.
    Named(Vec<String>),
    Auto,
    Select,
}

fn selector(value: &str) -> Selector {
    match value.trim().to_lowercase().as_str() {
        "auto" => Selector::Auto,
        "select" | "" => Selector::Select,
        _ => Selector::Named(
            value
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        ),
    }
}

/// Take cswap's own flags out of the argument list. Everything after a bare
/// `--` belongs to the agent, whatever it looks like. The last flag wins.
pub fn split_args(args: Vec<String>) -> (Option<Selector>, Vec<String>) {
    let mut sel = None;
    let mut rest = Vec::with_capacity(args.len());
    let mut literal = false;
    for arg in args {
        if literal {
            rest.push(arg);
        } else if arg == "--" {
            literal = true;
            rest.push(arg);
        } else if arg == "--cswap" || arg == "--cswap-select" {
            sel = Some(Selector::Select);
        } else if let Some(value) = arg.strip_prefix("--cswap=") {
            sel = Some(selector(value));
        } else {
            rest.push(arg);
        }
    }
    (sel, rest)
}

/// How the account for this launch was decided, for the one-line notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    Flag,
    Env,
    Default,
    Auto,
    Picker,
}

fn usable(acc: &Account, settings: &Settings) -> bool {
    suggest::rank(&[acc], settings, Utc::now())
        .first()
        .is_some_and(|p| p.usable)
}

fn pick_auto<'a>(
    reg: &'a Registry,
    h: Harness,
    settings: &Settings,
) -> Result<(&'a Account, String)> {
    let accounts: Vec<&Account> = reg.of(h).collect();
    if accounts.is_empty() {
        bail!("no {} account to pick from", h.name());
    }
    let ranked = suggest::rank(&accounts, settings, Utc::now());
    let top = ranked.first().expect("not empty");
    let acc = reg.get(&top.key).expect("ranked from the registry");
    let note = if top.usable {
        top.headline.clone()
    } else {
        format!("nothing is free right now; least bad: {}", top.headline)
    };
    Ok((acc, note))
}

fn resolve<'a>(
    reg: &'a Registry,
    h: Harness,
    sel: &Selector,
    settings: &Settings,
) -> Result<(&'a Account, Option<String>)> {
    match sel {
        Selector::Auto => pick_auto(reg, h, settings).map(|(a, n)| (a, Some(n))),
        Selector::Select => {
            let key = picker::pick(reg, h, settings)?.context("nothing selected")?;
            Ok((reg.get(&key).context("picked account vanished")?, None))
        }
        Selector::Named(names) => {
            let found: Vec<&Account> = names
                .iter()
                .map(|n| reg.find(h, n))
                .collect::<Result<_>>()?;
            let first = *found.first().context("`--cswap=` needs an account")?;
            if found.len() == 1 {
                return Ok((first, None));
            }
            match found.iter().find(|a| usable(a, settings)) {
                Some(a) => Ok((a, None)),
                None => Ok((
                    first,
                    Some("none of the listed accounts is free; using the first".to_string()),
                )),
            }
        }
    }
}

/// Dim one-line notice on stderr: which account this launch runs on.
fn announce(acc: &Account, via: Via, note: Option<&str>, settings: &Settings) {
    let now = Utc::now();
    let mut parts = vec![format!("{} #{}", acc.harness.id(), acc.meta.id)];
    let title = acc.title();
    if title != acc.key() {
        parts.push(title.clone());
    }
    if !acc.email().is_empty() && acc.email() != title {
        parts.push(acc.email().to_string());
    }
    if let Some(w) = acc.snapshot().and_then(|s| s.weekly()) {
        let mut text = format!("{} {:.0}%", w.period(), w.used_now(now));
        if let Some(t) = w.resets_at.filter(|t| *t > now) {
            text.push_str(&format!(
                ", resets {}",
                model::landing(t, settings.clock_24h)
            ));
        }
        parts.push(text);
    }
    let how = match via {
        Via::Flag => "",
        Via::Env => " (this terminal)",
        Via::Default => " (default)",
        Via::Auto => " (auto)",
        Via::Picker => "",
    };
    let line = format!("cswap ▸ {}{how}", parts.join(" · "));
    let color = std::env::var_os("NO_COLOR").is_none();
    let (dim, reset) = if color {
        ("\x1b[2m", "\x1b[0m")
    } else {
        ("", "")
    };
    eprintln!("{dim}{line}{reset}");
    if let Some(note) = note {
        eprintln!("{dim}        {note}{reset}");
    }
}

/// `cswap <harness> [args…]`: decide the account and become the agent.
pub fn run(h: Harness, args: Vec<String>) -> Result<()> {
    let (flag, rest) = split_args(args);
    let hs = HarnessSettings::load(h)?;
    let env_var = format!("CSWAP_{}", h.id().to_uppercase());
    let from_env = std::env::var(&env_var)
        .ok()
        .filter(|v| !v.trim().is_empty());

    let (sel, via) = match (flag, from_env) {
        (Some(Selector::Auto), _) => (Selector::Auto, Via::Auto),
        (Some(Selector::Select), _) => (Selector::Select, Via::Picker),
        (Some(s), _) => (s, Via::Flag),
        (None, Some(v)) => (selector(&v), Via::Env),
        (None, None) => match hs.start.as_str() {
            "auto" => (Selector::Auto, Via::Auto),
            other => (Selector::Named(vec![other.to_string()]), Via::Default),
        },
    };

    let settings = Settings::load()?;
    let reg = Registry::load()?;
    let (acc, note) = match resolve(&reg, h, &sel, &settings) {
        Ok(found) => found,
        // A stored default that no longer resolves (the account was removed by
        // hand) must not lock the user out of the agent: fall back to its
        // default login.
        Err(e) if via == Via::Default || via == Via::Env => {
            let native = reg
                .of(h)
                .find(|a| a.is_native())
                .with_context(|| format!("{e:#}"))?;
            eprintln!("cswap: {e:#}; using {}'s root login", h.spec().bins[0]);
            (native, None)
        }
        Err(e) => return Err(e),
    };
    launch(acc.clone(), &hs, &rest, via, note.as_deref(), &settings)
}

fn launch(
    mut acc: Account,
    hs: &HarnessSettings,
    args: &[String],
    via: Via,
    note: Option<&str>,
    settings: &Settings,
) -> Result<()> {
    let h = acc.harness;
    let bin = manage::find_bin(h, hs)?;
    if !acc.is_native() {
        if !h.wrappable() {
            bail!("{} cannot run on a cswap account", h.name());
        }
        let report = manage::prepare(&mut acc, hs)?;
        let shadowed = report.names(State::Shadowed);
        if !shadowed.is_empty() {
            eprintln!(
                "cswap: {} keeps its own copy of: {}",
                acc.key(),
                shadowed.join(", ")
            );
            eprintln!(
                "cswap: what the agent writes there is not shared. See `cswap sync`, fix with `cswap sync --repair`."
            );
        }
        if !h.logged_in(&acc.config_dir()) {
            bail!(
                "{} ({}) is not signed in. Run: cswap login {}",
                acc.key(),
                acc.title(),
                acc.key()
            );
        }
        h.harden(&acc.config_dir());
        // Best effort: a launch must not fail because a timestamp did not save.
        let _ = acc.update(|m| m.last_used = Some(Utc::now()));
    }
    // A notice only when a person is watching and cswap chose something other
    // than the agent's default login.
    let quiet = acc.is_native() && matches!(via, Via::Default);
    if hs.announce && !quiet && std::io::stderr().is_terminal() {
        announce(&acc, via, note, settings);
    }
    let mut cmd = Command::new(&bin);
    manage::apply_env(&mut cmd, &acc);
    cmd.args(args);
    sessions::register(&acc.key());
    match platform::become_program(cmd)? {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn cswap_flags_are_removed_and_everything_else_is_kept_in_order() {
        let (sel, rest) = split_args(s(&["-r", "--cswap=2", "--model", "opus"]));
        assert_eq!(sel, Some(Selector::Named(s(&["2"]))));
        assert_eq!(rest, s(&["-r", "--model", "opus"]));
    }

    #[test]
    fn select_auto_and_preference_lists() {
        assert_eq!(split_args(s(&["--cswap-select"])).0, Some(Selector::Select));
        // The flag alone asks too, and the rest still goes to the agent.
        let (sel, rest) = split_args(s(&["--cswap", "-r"]));
        assert_eq!((sel, rest), (Some(Selector::Select), s(&["-r"])));
        // After a bare `--` it is the agent's.
        assert_eq!(split_args(s(&["--", "--cswap"])).0, None);
        assert_eq!(split_args(s(&["--cswap=select"])).0, Some(Selector::Select));
        assert_eq!(split_args(s(&["--cswap=auto"])).0, Some(Selector::Auto));
        // Only the documented flags are cswap's. Anything else is the agent's.
        let (sel, rest) = split_args(s(&["--cswap-auto", "--cswapx=1"]));
        assert_eq!(sel, None);
        assert_eq!(rest, s(&["--cswap-auto", "--cswapx=1"]));
        assert_eq!(
            split_args(s(&["--cswap=work, 3,default"])).0,
            Some(Selector::Named(s(&["work", "3", "default"])))
        );
    }

    #[test]
    fn nothing_after_a_bare_double_dash_is_ours() {
        let (sel, rest) = split_args(s(&["-p", "--", "--cswap=2"]));
        assert_eq!(sel, None);
        assert_eq!(rest, s(&["-p", "--", "--cswap=2"]));
    }

    #[test]
    fn the_last_flag_wins_and_lookalikes_pass_through() {
        let (sel, rest) = split_args(s(&["--cswap=1", "--cswapper", "--cswap=2"]));
        assert_eq!(sel, Some(Selector::Named(s(&["2"]))));
        assert_eq!(rest, s(&["--cswapper"]));
    }
}
