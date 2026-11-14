//! Herdr does not know which conversation a terminal held. When it is stopped
//! (`hc server stop`, a reboot), every program in every terminal ends: a kept
//! terminal comes back as an empty shell, and a worker that was not kept does
//! not come back at all. Which agent, account and conversation was where then
//! has to be pieced together from logs.
//!
//! So cswap writes that down while everything runs (`cswap herdr save`), and
//! puts it back afterwards (`cswap herdr restore`): an agent whose terminal
//! is there again is started in it, in its conversation, and a terminal that
//! is gone is started again with `hc run`, under the parent it had.
//!
//! The layout asks for a save every minute, with `--auto`, and cswap writes
//! one when the last is older than the setting says (ten minutes unless
//! changed; 0 turns it off). Right after a stop that would write an empty
//! list over the one save worth having, so an automatic save leaves the file
//! alone when herdr's core started after the file was written and an agent
//! in it does not run where it did.
//! A restore, or a save by hand, starts the saving again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli::Ctx;
use crate::harness::Harness;
use crate::herdr::{self, hc, Terminal};
use crate::sessions::{self, Talk};
use crate::{fsx, paths};

/// One agent as it ran: enough to start it again where it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The terminal's address when saved. A terminal that is started again
    /// gets a new one.
    pub terminal: String,
    pub name: Option<String>,
    pub agent: String,
    /// The account's key, and its title for a person to read.
    pub account: String,
    pub account_name: String,
    /// `open`, `fresh` or `unknown`: see `sessions::Talk`.
    pub conversation: String,
    pub session: Option<String>,
    /// The arguments that open the conversation again.
    pub resume: Vec<String>,
    /// The model and the like, from the launch that was running.
    pub options: Vec<String>,
    /// The folder the agent ran in.
    pub cwd: Option<String>,
    pub parent: Option<String>,
    pub keep: bool,
    pub outlive: bool,
    pub notify: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Seconds since 1970.
    pub saved_at: i64,
    pub terminals: Vec<Entry>,
}

fn file(previous: bool) -> PathBuf {
    paths::root().join(if previous {
        "herdr-saved.prev.json"
    } else {
        "herdr-saved.json"
    })
}

/// When herdr's core started, in seconds since 1970. A stop of herdr or of
/// the machine ends every program, and the core that runs afterwards is a
/// new one. An error when herdr does not say: a guess here would let an
/// empty list replace the one a restore needs.
fn core_started(now: i64) -> Result<i64> {
    let v: Value = serde_json::from_str(&hc(&["server", "status", "--json"])?)
        .context("hc server status did not print JSON")?;
    let up = v["uptime_ms"]
        .as_i64()
        .context("hc server status did not say how long herdr has been running")?;
    Ok(now - up / 1000)
}

/// What a save came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrote {
    Yes,
    /// The file is from before a stop and lists agents: it is what a restore
    /// needs, and what runs now is less.
    Kept,
    /// Automatic saving is turned off.
    Off,
    /// The last save is newer than the time between saves.
    NotDue,
}

impl Wrote {
    fn word(self) -> &'static str {
        match self {
            Wrote::Yes => "yes",
            Wrote::Kept => "kept",
            Wrote::Off => "off",
            Wrote::NotDue => "not_due",
        }
    }
}

/// What herdr and this machine say now, for an automatic save to go by.
struct Seen {
    now: i64,
    /// When herdr's core started.
    started: i64,
    /// The agent in each terminal: address and agent id.
    running: Vec<(String, String)>,
}

/// Is the file from before a stop, with agents that are not back yet? Both
/// must hold. The core started after the file was written: nothing else
/// ends every agent at once. And an agent in the file does not run where it
/// did: a core that was only restarted keeps its programs, and after a
/// restore by hand there is nothing left to bring back.
fn pending(old: Option<&Snapshot>, seen: &Seen) -> bool {
    old.is_some_and(|o| {
        seen.started >= o.saved_at
            && o.terminals.iter().any(|e| {
                !seen
                    .running
                    .contains(&(e.terminal.clone(), e.agent.clone()))
            })
    })
}

/// What an automatic save does now, `minutes` being the time between saves.
fn automatic(old: Option<&Snapshot>, seen: &Seen, minutes: u64) -> Wrote {
    if pending(old, seen) {
        Wrote::Kept
    } else if minutes == 0 {
        Wrote::Off
    } else if old.is_some_and(|o| seen.now - o.saved_at < minutes as i64 * 60) {
        Wrote::NotDue
    } else {
        Wrote::Yes
    }
}

/// Every agent in a herdr terminal, as it runs now.
fn taken(c: &Ctx) -> Vec<Entry> {
    herdr::in_terminals(c)
        .into_iter()
        .filter_map(|(address, pid, acc)| {
            // A terminal herdr no longer has: the agent is on its way out.
            let term = Terminal::get(&address).ok()?;
            let talk = sessions::talk(acc.harness, pid, &acc.config_dir());
            let (session, resume) = match &talk {
                Talk::Open { id, resume } => (Some(id.clone()), resume.clone()),
                _ => (None, Vec::new()),
            };
            let cwd = sessions::details(pid)
                .cwd
                .map(|p| p.to_string_lossy().to_string())
                .or(term.cwd);
            Some(Entry {
                terminal: term.address,
                name: term.name,
                agent: acc.harness.id().to_string(),
                account: acc.key(),
                account_name: acc.title(),
                conversation: talk.word().to_string(),
                session,
                resume,
                options: herdr::kept_options(acc.harness, &sessions::command_line(pid)),
                cwd,
                parent: term.parent,
                keep: term.keep,
                outlive: term.outlive,
                notify: term.notify,
            })
        })
        .collect()
}

/// What a save did.
pub struct Saved {
    /// The file as it is now; none when there is none.
    pub snapshot: Option<Snapshot>,
    pub wrote: Wrote,
}

pub fn save(c: &Ctx, auto: bool) -> Result<Saved> {
    let now = c.now.timestamp();
    // Asked also for a save by hand: it fails here when herdr is not running.
    let started = core_started(now)?;
    let old: Option<Snapshot> = fsx::read_json(&file(false))?;
    let wrote = if auto {
        let seen = Seen {
            now,
            started,
            running: herdr::in_terminals(c)
                .into_iter()
                .map(|(address, _, acc)| (address, acc.harness.id().to_string()))
                .collect(),
        };
        automatic(old.as_ref(), &seen, c.settings.herdr_save_minutes)
    } else {
        Wrote::Yes
    };
    if wrote != Wrote::Yes {
        return Ok(Saved {
            snapshot: old,
            wrote,
        });
    }
    let snapshot = Snapshot {
        saved_at: now,
        terminals: taken(c),
    };
    // A save by hand, or the one after a restore, replaces a file that may
    // list more than runs now: that one stays within reach.
    if !auto
        && old
            .as_ref()
            .is_some_and(|o| o.terminals != snapshot.terminals)
    {
        fsx::write_json(&file(true), &old)?;
    }
    fsx::write_json(&file(false), &snapshot)?;
    Ok(Saved {
        snapshot: Some(snapshot),
        wrote,
    })
}

/// How a restore is to go about it.
#[derive(Debug, Clone, Copy, Default)]
pub struct How {
    pub dry_run: bool,
    pub fresh: bool,
    pub previous: bool,
}

/// What became of one saved agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Did {
    /// Typed into the shell of its own terminal.
    Typed { started: bool },
    /// Its terminal was gone, and a new one was started.
    Run { address: String },
    /// Left alone, and why. Nothing is wrong.
    Skipped(String),
    /// It should have come back and did not.
    Failed(String),
}

pub struct Outcome {
    pub entry: Entry,
    pub did: Did,
    /// What was typed, or the `hc run` that was given.
    pub line: String,
    /// The line that starts it by hand, in a shell of your own.
    pub manual: String,
}

/// The line a person can paste into a shell to start the agent of `e`.
fn manual_line(c: &Ctx, e: &Entry) -> String {
    let selector = c
        .reg
        .get(&e.account)
        .map_or_else(|| e.account.clone(), herdr::selector);
    let launch = quoted(&launch_words(e, &selector));
    match &e.cwd {
        Some(cwd) => format!("cd {} && {launch}", herdr::quote(cwd)),
        None => launch,
    }
}

/// The entries with every parent before its children, in their saved order
/// otherwise. A parent that is not in the list counts as there.
fn parents_first(entries: &[Entry]) -> Vec<&Entry> {
    let mut out: Vec<&Entry> = Vec::new();
    let mut left: Vec<&Entry> = entries.iter().collect();
    while !left.is_empty() {
        let waits = |e: &Entry, left: &[&Entry]| {
            e.parent.as_ref().is_some_and(|p| {
                left.iter()
                    .any(|l| &l.terminal == p && l.terminal != e.terminal)
            })
        };
        let (ready, rest): (Vec<&Entry>, Vec<&Entry>) = left.iter().partition(|e| !waits(e, &left));
        // Parents in a ring: herdr makes none, but a file can say anything.
        if ready.is_empty() {
            out.extend(rest);
            break;
        }
        out.extend(ready);
        left = rest;
    }
    out
}

/// The words that start the agent of `e` on `selector`.
fn launch_words(e: &Entry, selector: &str) -> Vec<String> {
    let mut words = vec![
        "cswap".to_string(),
        e.agent.clone(),
        format!("--cswap={selector}"),
    ];
    words.extend(e.options.iter().cloned());
    words.extend(e.resume.iter().cloned());
    words
}

fn quoted(words: &[String]) -> String {
    words
        .iter()
        .map(|w| herdr::quote(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Bring one saved agent back. `moved` says where a terminal that was
/// started again is now.
fn restore_one(c: &Ctx, e: &Entry, moved: &HashMap<String, String>, how: How) -> (Did, String) {
    let skip = |why: String| (Did::Skipped(why), String::new());
    let Some(acc) = Harness::from_id(&e.agent).and_then(|_| c.reg.get(&e.account)) else {
        return skip(format!("the account {} is gone", e.account_name));
    };
    if !c.signed_in(acc) {
        return skip(format!("{} is not signed in", acc.title()));
    }
    if e.conversation == "unknown" && !how.fresh {
        return skip(
            "its conversation was not known when it was saved: `--fresh` starts a new one"
                .to_string(),
        );
    }
    if let Some(cwd) = e.cwd.as_deref().filter(|d| !Path::new(d).is_dir()) {
        return skip(format!("its folder is gone: {cwd}"));
    }
    let words = launch_words(e, &herdr::selector(acc));

    // Its own terminal is there: the agent goes back into it.
    if let Some(term) = Terminal::get(&e.terminal).ok().filter(|t| t.live) {
        if let Some((there, _)) = term.front.and_then(|pid| herdr::agent_of(c, pid)) {
            return skip(format!("{} already runs there", there.harness.name()));
        }
        if !term.front_shell {
            return skip("another program is running there".to_string());
        }
        let line = manual_line(c, e);
        if how.dry_run {
            return (Did::Typed { started: false }, line);
        }
        let did = match herdr::launch(c, &term.address, &line, &acc.key(), true) {
            Ok(started) => Did::Typed { started },
            Err(e) => Did::Failed(e.to_string()),
        };
        return (did, line);
    }

    // It is gone: a new terminal, under the parent it had if that is there.
    if let Some(name) = &e.name {
        if Terminal::get(name).is_ok_and(|t| t.live) {
            return skip(format!("a terminal named {name} is running"));
        }
    }
    let mut args: Vec<String> = ["run", "--bg", "--json"].map(String::from).to_vec();
    let mut flag = |name: &str, value: Option<&str>| {
        args.push(name.to_string());
        args.extend(value.map(str::to_string));
    };
    if let Some(name) = &e.name {
        flag("--name", Some(name));
    }
    if let Some(cwd) = &e.cwd {
        flag("--cwd", Some(cwd));
    }
    let parent = e
        .parent
        .as_ref()
        .map(|p| moved.get(p).unwrap_or(p))
        .filter(|p| Terminal::get(p).is_ok_and(|t| t.live));
    match parent {
        Some(p) => flag("--parent", Some(p)),
        // Without this it would become a child of whoever runs the restore.
        None => flag("--no-parent", None),
    }
    if e.outlive && parent.is_some() {
        flag("--outlive-parent", None);
    }
    if e.keep {
        flag("--keep", None);
    }
    if !e.notify {
        flag("--no-notify", None);
    }
    args.extend(words);
    let line = format!("hc {}", quoted(&args));
    if how.dry_run {
        return (
            Did::Run {
                address: String::new(),
            },
            line,
        );
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let did = match hc(&refs) {
        Ok(out) => match serde_json::from_str::<Value>(&out)
            .ok()
            .and_then(|v| v["address"].as_str().map(str::to_string))
        {
            Some(address) => Did::Run { address },
            None => Did::Failed("hc run did not say which terminal it started".to_string()),
        },
        Err(e) => Did::Failed(e.to_string()),
    };
    (did, line)
}

/// Bring the saved agents back. `only` picks some by address or name.
pub fn restore(c: &Ctx, only: &[String], how: How) -> Result<(Snapshot, Vec<Outcome>)> {
    let path = file(how.previous);
    let snapshot: Snapshot = fsx::read_json(&path)?.with_context(|| {
        format!(
            "nothing is saved in {}: `cswap herdr save` writes it while the agents run",
            path.display()
        )
    })?;
    // Fails here when herdr is not running, before anything is tried.
    core_started(c.now.timestamp())?;
    let picked = |e: &Entry| {
        only.is_empty()
            || only
                .iter()
                .any(|o| *o == e.terminal || Some(o) == e.name.as_ref())
    };
    if let Some(missing) = only.iter().find(|o| {
        !snapshot
            .terminals
            .iter()
            .any(|e| **o == e.terminal || Some(*o) == e.name.as_ref())
    }) {
        bail!("{missing} is not in the save");
    }
    let mut moved: HashMap<String, String> = HashMap::new();
    let mut out = Vec::new();
    for e in parents_first(&snapshot.terminals) {
        if !picked(e) {
            continue;
        }
        let (did, line) = restore_one(c, e, &moved, how);
        if let Did::Run { address } = &did {
            if !address.is_empty() {
                moved.insert(e.terminal.clone(), address.clone());
            }
        }
        out.push(Outcome {
            manual: manual_line(c, e),
            entry: e.clone(),
            did,
            line,
        });
    }
    Ok((snapshot, out))
}

// ------------------------------------------------------------------- commands

fn ago(c: &Ctx, at: i64) -> String {
    let secs = (c.now.timestamp() - at).max(0);
    match secs {
        0..=89 => "just now".to_string(),
        90..=5399 => format!("{} min ago", (secs + 30) / 60),
        5400..=172_799 => format!("{} h ago", (secs + 1800) / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// The agent's name as a person knows it.
fn agent_name(e: &Entry) -> String {
    Harness::from_id(&e.agent).map_or_else(|| e.agent.clone(), |h| h.name().to_string())
}

/// `t107 build-r1`, or the address alone.
fn called(e: &Entry) -> String {
    match &e.name {
        Some(name) => format!("{} {name}", e.terminal),
        None => e.terminal.clone(),
    }
}

/// `cswap herdr save`.
pub fn save_cmd(auto: bool, as_json: bool) -> Result<()> {
    let c = Ctx::load()?;
    let s = save(&c, auto)?;
    let terminals = s.snapshot.as_ref().map_or(&[][..], |f| &f.terminals);
    let saved_at = s.snapshot.as_ref().map(|f| f.saved_at);
    if as_json {
        let v = json!({
            "file": file(false),
            "wrote": s.wrote.word(),
            // Agents from before a stop that a restore would bring back.
            "pending": s.wrote == Wrote::Kept,
            "every_minutes": c.settings.herdr_save_minutes,
            "saved_at": saved_at,
            "now": c.now.timestamp(),
            "agents": terminals.len(),
            "terminals": terminals,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let n = terminals.len();
    let agents = if n == 1 {
        "1 agent".to_string()
    } else {
        format!("{n} agents")
    };
    let when = saved_at.map_or_else(|| "never".to_string(), |at| ago(&c, at));
    match s.wrote {
        Wrote::Yes => println!("Saved {agents} in {}.", file(false).display()),
        Wrote::Kept => {
            println!(
                "Left the save of {agents} from {when} as it is: herdr was stopped since, and \
                 they are not back yet."
            );
            println!(
                "{}",
                c.p.dim("`cswap herdr restore` brings them back; `cswap herdr save` without --auto saves what runs now.")
            );
            return Ok(());
        }
        Wrote::Off => {
            println!("Automatic saving is off. Last save: {when}.");
            return Ok(());
        }
        Wrote::NotDue => {
            println!(
                "Not due: the last save is from {when}, and cswap saves every {} min.",
                c.settings.herdr_save_minutes
            );
            return Ok(());
        }
    }
    for e in terminals {
        let talk = match (&e.session, e.conversation.as_str()) {
            (Some(id), _) => format!("conversation {id}"),
            (None, "fresh") => "nothing said yet".to_string(),
            _ => "conversation not known".to_string(),
        };
        println!(
            "{}  {} on {} · {}",
            c.p.bold(&format!("{:<6}", e.terminal)),
            agent_name(e),
            e.account_name,
            c.p.dim(&talk)
        );
    }
    Ok(())
}

/// `cswap herdr restore`. False when an agent should have come back and did
/// not.
pub fn restore_cmd(only: &[String], how: How, as_json: bool) -> Result<bool> {
    let c = Ctx::load()?;
    let (snapshot, done) = restore(&c, only, how)?;
    let failed = |d: &Did| matches!(d, Did::Failed(_) | Did::Typed { started: false });
    let ok = how.dry_run || !done.iter().any(|o| failed(&o.did));
    // What runs now is the list to keep from here on. The file that was
    // restored from stays as the previous one.
    if !how.dry_run && only.is_empty() && !how.previous {
        save(&c, false)?;
    }
    if as_json {
        let rows: Vec<Value> = done
            .iter()
            .map(|o| {
                let (what, note, address, started) = match &o.did {
                    Did::Typed { started } => (
                        "typed",
                        None,
                        Some(o.entry.terminal.clone()),
                        Some(*started),
                    ),
                    Did::Run { address } => (
                        "run",
                        None,
                        Some(address.clone()).filter(|a| !a.is_empty()),
                        None,
                    ),
                    Did::Skipped(why) => ("skipped", Some(why.clone()), None, None),
                    Did::Failed(why) => ("failed", Some(why.clone()), None, None),
                };
                json!({
                    "terminal": o.entry.terminal,
                    "name": o.entry.name,
                    "agent": o.entry.agent,
                    "agent_name": agent_name(&o.entry),
                    "account": o.entry.account_name,
                    "session": o.entry.session,
                    "did": what,
                    "now": address,
                    "started": started,
                    "why": note,
                    "line": o.line,
                    "manual": o.manual,
                    "cwd": o.entry.cwd,
                })
            })
            .collect();
        let v = json!({
            "saved_at": snapshot.saved_at,
            "now": c.now.timestamp(),
            "dry_run": how.dry_run,
            "previous": how.previous,
            "ok": ok,
            "terminals": rows,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(ok);
    }
    println!(
        "{} from the save of {}:",
        if how.dry_run {
            "What a restore would do"
        } else {
            "Restored"
        },
        ago(&c, snapshot.saved_at)
    );
    let wide = done
        .iter()
        .map(|o| called(&o.entry).len())
        .max()
        .unwrap_or(0);
    for o in &done {
        let e = &o.entry;
        let who = format!("{} on {}", agent_name(e), e.account_name);
        let talk = match &e.session {
            Some(_) => "same conversation",
            None => "new conversation",
        };
        let what = match &o.did {
            Did::Typed { .. } if how.dry_run => format!("{who}: start it in its terminal, {talk}"),
            Did::Run { .. } if how.dry_run => format!("{who}: start a new terminal for it, {talk}"),
            Did::Typed { started: true } => format!("{who}, {talk}"),
            Did::Typed { started: false } => c.p.warn(&format!(
                "{who}: the launch was typed, but the agent is not seen running yet: look at the terminal"
            )),
            Did::Run { address } => format!("{who}, {talk}, now {address}"),
            Did::Skipped(why) => c.p.dim(&format!("left alone: {why}")),
            Did::Failed(why) => c.p.warn(&format!("{who}: not started: {why}")),
        };
        println!("{}  {what}", c.p.bold(&format!("{:<wide$}", called(e))));
        // By hand: the same line works in any shell of your own.
        if how.dry_run {
            let line = if o.line.is_empty() {
                &o.manual
            } else {
                &o.line
            };
            println!("{:<wide$}  {}", "", c.p.dim(line));
        }
    }
    if done.is_empty() {
        println!("{}", c.p.dim("The save lists no agent."));
    }
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(terminal: &str, parent: Option<&str>) -> Entry {
        Entry {
            terminal: terminal.to_string(),
            name: None,
            agent: "claude".to_string(),
            account: "claude-1".to_string(),
            account_name: "work".to_string(),
            conversation: "open".to_string(),
            session: Some("abc".to_string()),
            resume: vec!["--resume".to_string(), "abc".to_string()],
            options: vec!["--model".to_string(), "opus".to_string()],
            cwd: None,
            parent: parent.map(str::to_string),
            keep: false,
            outlive: true,
            notify: true,
        }
    }

    /// A file written at second 1000.
    fn snap(terminals: Vec<Entry>) -> Snapshot {
        Snapshot {
            saved_at: 1000,
            terminals,
        }
    }

    /// Herdr's core started at `started`, and Claude Code runs in `running`.
    fn seen(now: i64, started: i64, running: &[&str]) -> Seen {
        Seen {
            now,
            started,
            running: running
                .iter()
                .map(|t| (t.to_string(), "claude".to_string()))
                .collect(),
        }
    }

    #[test]
    fn an_automatic_save_keeps_the_list_from_before_a_stop() {
        let full = snap(vec![entry("t1", None), entry("t2", Some("t1"))]);
        // herdr started after the save and an agent is not back: kept, with
        // the timer off too, and however old the file is.
        assert_eq!(
            automatic(Some(&full), &seen(99_999, 2000, &["t1"]), 10),
            Wrote::Kept
        );
        assert_eq!(
            automatic(Some(&full), &seen(99_999, 2000, &[]), 0),
            Wrote::Kept
        );
        // Everything is back, by a restore or by hand: saving goes on.
        assert_eq!(
            automatic(Some(&full), &seen(99_999, 2000, &["t1", "t2"]), 10),
            Wrote::Yes
        );
        // Nothing worth keeping.
        assert_eq!(
            automatic(Some(&snap(vec![])), &seen(99_999, 2000, &[]), 10),
            Wrote::Yes
        );
        assert_eq!(automatic(None, &seen(99_999, 2000, &[]), 10), Wrote::Yes);
    }

    /// Saving once stopped for good: a file that only looked like one from
    /// before a stop was never written over.
    #[test]
    fn an_agent_that_left_does_not_stop_the_saving() {
        let full = snap(vec![entry("t1", None), entry("t2", Some("t1"))]);
        // The same core as at the save: t2 just ended.
        assert_eq!(
            automatic(Some(&full), &seen(1600, 500, &["t1"]), 10),
            Wrote::Yes
        );
        assert_eq!(
            automatic(Some(&full), &seen(1600, 500, &[]), 10),
            Wrote::Yes
        );
    }

    #[test]
    fn an_automatic_save_waits_for_the_time_between_saves() {
        let full = snap(vec![entry("t1", None)]);
        let at = |now| seen(now, 500, &["t1"]);
        assert_eq!(automatic(Some(&full), &at(1599), 10), Wrote::NotDue);
        assert_eq!(automatic(Some(&full), &at(1600), 10), Wrote::Yes);
        assert_eq!(automatic(Some(&full), &at(1060), 1), Wrote::Yes);
        // Turned off: only a save by hand writes.
        assert_eq!(automatic(Some(&full), &at(99_999), 0), Wrote::Off);
        assert_eq!(automatic(None, &at(0), 0), Wrote::Off);
    }

    #[test]
    fn a_parent_is_restored_before_its_children() {
        let all = [
            entry("t3", Some("t2")),
            entry("t2", Some("t1")),
            entry("t1", None),
            // Its parent is not in the save.
            entry("t4", Some("t9")),
        ];
        let order: Vec<&str> = parents_first(&all)
            .iter()
            .map(|e| e.terminal.as_str())
            .collect();
        assert_eq!(order, ["t1", "t4", "t2", "t3"]);
        // A ring ends, with everything in it.
        let ring = [entry("t1", Some("t2")), entry("t2", Some("t1"))];
        assert_eq!(parents_first(&ring).len(), 2);
    }

    #[test]
    fn the_launch_carries_the_account_the_options_and_the_conversation() {
        let words = launch_words(&entry("t1", None), "work");
        assert_eq!(
            quoted(&words),
            "cswap claude --cswap=work --model opus --resume abc"
        );
    }
}
