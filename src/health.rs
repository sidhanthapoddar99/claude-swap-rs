//! Checks for the failures that are otherwise silent: a missing binary, a
//! shell that does not route through cswap, a login that has run out, a real
//! file hiding a shared one. The dashboard's Health page and `cswap doctor`
//! both show this list.

use chrono::Utc;
use std::fs;

use crate::harness::Harness;
use crate::sessions::Live;
use crate::store::{self, HarnessSettings, Registry};
use crate::{manage, model, paths, shell};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Info,
    Warn,
    Bad,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub level: Level,
    /// What the check is about: `cswap`, `shims`, an agent's id or an
    /// account's key.
    pub area: String,
    /// The heading it is listed under: an agent's checks and its accounts'
    /// share one. Checks of one group follow each other.
    pub group: String,
    /// The agent the group is about, when it is about one.
    pub agent: Option<Harness>,
    pub title: String,
    /// What to do about it; empty when nothing needs doing.
    pub detail: String,
}

fn check(level: Level, area: &str, title: impl Into<String>, detail: impl Into<String>) -> Check {
    // An account's key is its agent's id and a number: `claude-2`.
    let agent = Harness::ALL.into_iter().find(|h| {
        area.strip_prefix(h.id())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
    });
    let group = match (agent, area) {
        (Some(h), _) => h.name(),
        (None, "shims") => "PATH shims",
        (None, other) => other,
    };
    Check {
        level,
        area: area.to_string(),
        group: group.to_string(),
        agent,
        title: title.into(),
        detail: detail.into(),
    }
}

pub fn run(reg: &Registry, live: &Live) -> Vec<Check> {
    let mut out = Vec::new();
    let now = Utc::now();

    // ---- cswap itself
    let root = paths::root();
    match fs::metadata(&root) {
        Ok(md) if crate::platform::others_can_read(&md) => out.push(check(
            Level::Warn,
            "cswap",
            format!("{} is readable by other users", paths::tilde(&root)),
            format!("It holds logins. Run: chmod 700 {}", paths::tilde(&root)),
        )),
        Ok(_) => out.push(check(
            Level::Ok,
            "cswap",
            format!("{} is private to you", paths::tilde(&root)),
            "",
        )),
        Err(_) => out.push(check(
            Level::Info,
            "cswap",
            "no cswap data yet",
            "Add an account to get started.",
        )),
    }
    let old = crate::migrate::pending().len();
    if old > 0 {
        out.push(check(
            Level::Info,
            "cswap",
            format!(
                "{old} profile{} from cswap 0.6 can be brought over",
                if old == 1 { "" } else { "s" }
            ),
            "Run: cswap migrate. Each becomes a Claude account and keeps its names.",
        ));
    }
    let name = shell::current();
    match (name.clone(), shell::loaded(&name)) {
        (name, Some(true)) => out.push(check(
            Level::Ok,
            "shell",
            format!("{name} loads the cswap wrappers"),
            "",
        )),
        (name, Some(false)) => out.push(check(
            Level::Warn,
            "shell",
            format!("{name} does not load the cswap wrappers"),
            format!(
                "`claude`, `codex` and the rest bypass cswap. Wrap them in the dashboard under Settings > Shell, or add to your rc file: {}",
                shell::line(&name)
            ),
        )),
        (name, None) => out.push(check(
            Level::Info,
            "shell",
            format!("shell '{name}' has no cswap integration"),
            "bash, zsh and fish are supported. Elsewhere, alias each agent to `cswap <agent>`.",
        )),
    }

    // ---- PATH shims: do agents that other programs start reach cswap?
    if crate::shim::supported() {
        use crate::shim::State;
        let settings = store::Settings::load().unwrap_or_default();
        let chosen = crate::shim::chosen(&settings);
        let herdr =
            crate::integration::herdr_home().is_dir() || crate::fsx::which("hc", None).is_some();
        let later = "A new terminal sees the new PATH when its shell loads cswap, and every program after your next login. herdr keeps the PATH it started with: to give it the new one now, run `hc server restart --fresh` in a terminal outside herdr.";
        if chosen.is_empty() && herdr {
            out.push(check(
                Level::Warn,
                "shims",
                "agents that herdr starts by a bare name skip cswap",
                "herdr runs `claude`, `codex` and the rest straight from PATH, so a sub-agent an agent starts with `hc run claude …` lands on the root login, not on the default you set in cswap. Turn PATH shims on: Settings > Shell > PATH shims, or `cswap shims on`. Without them, start sub-agents as `cswap claude …`.",
            ));
        }
        for h in &chosen {
            let bin = h.spec().bins[0];
            match crate::shim::state(*h) {
                State::Active => out.push(check(
                    Level::Ok,
                    "shims",
                    format!("a bare {bin} reaches cswap from this environment"),
                    "",
                )),
                State::Broken => out.push(check(
                    Level::Warn,
                    "shims",
                    format!("the shim for {bin} is missing or leads to another cswap"),
                    "Make it again: `cswap shims on`, or Settings > Shell > PATH shims.",
                )),
                State::Shadowed(first) => out.push(check(
                    Level::Warn,
                    "shims",
                    format!(
                        "{} comes before cswap's shim on PATH here",
                        paths::tilde(&first)
                    ),
                    format!(
                        "A bare `{bin}` started from this environment skips cswap. {later}"
                    ),
                )),
                State::OffPath => out.push(check(
                    Level::Warn,
                    "shims",
                    format!(
                        "{} is not on PATH here yet",
                        paths::tilde(&crate::shim::dir())
                    ),
                    format!("So a bare `{bin}` started from this environment is not found through it. {later}"),
                )),
            }
        }
    }

    // ---- PATH shims: and does herdr, which keeps the PATH it started with?
    if crate::shim::supported() {
        let settings = store::Settings::load().unwrap_or_default();
        let chosen: Vec<Harness> = crate::shim::chosen(&settings)
            .into_iter()
            .filter(|h| crate::shim::state(*h) != crate::shim::State::Broken)
            .collect();
        for (h, first, ours) in crate::shim::seen_by_herdr(&chosen).unwrap_or_default() {
            let bin = h.spec().bins[0];
            if ours {
                out.push(check(
                    Level::Ok,
                    "shims",
                    format!("herdr starts a bare {bin} through cswap"),
                    "",
                ));
                continue;
            }
            let what = match &first {
                Some(first) => format!("herdr still starts {}", paths::tilde(first)),
                None => format!("herdr finds no {bin} at all"),
            };
            out.push(check(
                Level::Warn,
                "shims",
                format!("{what}, not cswap's shim"),
                format!(
                    "herdr keeps the PATH it started with, and that one does not have {} first. Until it does, `hc run {bin} …` skips cswap. Give herdr the new PATH: run `hc server restart --fresh` in a terminal outside herdr, or log in again.",
                    paths::tilde(&crate::shim::dir())
                ),
            ));
        }
    }

    // ---- each harness
    for h in Harness::ALL {
        let hs = HarnessSettings::load(h).unwrap_or_default();
        let accounts: Vec<_> = reg.of(h).collect();
        match manage::find_bin(h, &hs) {
            Ok(bin) => out.push(check(
                Level::Ok,
                h.id(),
                format!("{} found at {}", h.name(), paths::tilde(&bin)),
                "",
            )),
            Err(_) if accounts.is_empty() => {
                out.push(check(
                    Level::Info,
                    h.id(),
                    format!("{} is not installed", h.name()),
                    "",
                ));
                continue;
            }
            Err(e) => out.push(check(
                Level::Bad,
                h.id(),
                format!("{} has accounts but no binary", h.name()),
                format!("{e:#}"),
            )),
        }
        if !h.wrappable() {
            out.push(check(
                Level::Info,
                h.id(),
                format!("{} cannot be moved to another config dir", h.name()),
                "cswap shows its root login but cannot hold accounts for it.",
            ));
        }
        if hs.start != "root" && hs.start != "auto" && reg.find(h, &hs.start).is_err() {
            out.push(check(
                Level::Warn,
                h.id(),
                format!(
                    "the default account ('{}') is gone",
                    hs.start
                ),
                "Launches fall back to the agent's root login. Pick another default on the Accounts page.",
            ));
        }

        for acc in accounts {
            let key = acc.key();
            let name = if acc.is_native() {
                format!("{}'s root login", h.name())
            } else {
                format!("{} ({key})", acc.title())
            };
            let dir = acc.config_dir();
            if !acc.is_native() && acc.meta.label.is_empty() {
                out.push(check(
                    Level::Warn,
                    &key,
                    format!("{key} has no name"),
                    format!("Every account has one. Run: cswap rename {key} <name>"),
                ));
            }
            // One login, one account. Said once for each pair.
            if let Some(other) = manage::duplicate_of(reg, acc)
                .filter(|o| !acc.is_native() && o.meta.id < acc.meta.id)
            {
                out.push(check(
                    Level::Warn,
                    &key,
                    format!("{name} holds the same login as {}", other.title()),
                    format!(
                        "A login kept in two accounts stops working in one as soon as the other renews it. Remove one: cswap remove {key}"
                    ),
                ));
            }
            if !h.logged_in(&dir) {
                if !acc.is_native() {
                    out.push(check(
                        Level::Warn,
                        &key,
                        format!("{name} is not signed in"),
                        format!("Run: cswap login {key}"),
                    ));
                }
                continue;
            }
            let running = live.get(&key).is_some_and(|p| !p.is_empty());
            let local = h.local(&dir, acc.is_native());
            match local.token_expires {
                Some(t) if t <= now && !running => out.push(check(
                    Level::Info,
                    &key,
                    format!("{name}: access token ran out {}", model::ago(t, now)),
                    format!(
                        "{} renews it the next time it runs on this account.",
                        h.name()
                    ),
                )),
                _ => {}
            }
            if acc.meta.refresh.needs_user {
                if let Some(e) = &acc.meta.refresh.error {
                    out.push(check(
                        Level::Warn,
                        &key,
                        format!("{name}: usage unavailable"),
                        e.clone(),
                    ));
                }
            }
            if !acc.meta.sync.shadowed.is_empty() {
                out.push(check(
                    Level::Warn,
                    &key,
                    format!(
                        "{name} keeps its own copy of {}",
                        acc.meta.sync.shadowed.join(", ")
                    ),
                    format!(
                        "What the agent writes there is not shared. Run: cswap sync {key} --repair"
                    ),
                ));
            }
            if acc.meta.billing.ended(store::today()) {
                out.push(check(
                    Level::Warn,
                    &key,
                    format!("{name}: the plan has ended"),
                    "Update the renewal date on the Accounts page, or remove the account.",
                ));
            }
        }
    }
    out
}

/// The worst level in a list, for a one-glance summary.
pub fn worst(checks: &[Check]) -> Level {
    checks.iter().map(|c| c.level).max().unwrap_or(Level::Ok)
}

/// How many checks want attention.
pub fn attention(checks: &[Check]) -> usize {
    checks.iter().filter(|c| c.level >= Level::Warn).count()
}
