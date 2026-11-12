//! Herdr keeps agents in terminals that stay alive. This is what cswap does
//! for it: say which account the agent in a terminal runs on, list what a
//! launch menu can offer, and move a running agent to another account without
//! losing its conversation, or start it again on the account it is on.
//!
//! A switch stops the agent, waits for a shell in its place, and types the
//! launch on the new account with the arguments that resume the conversation.
//! The terminal, its tab, its place and its sub-agents stay as they are.
//! A restart is the same on the account the agent already runs on: it comes
//! back as the program on disk now, with its settings read again.
//!
//! Between the agent leaving and the launch being typed the terminal is a
//! bare shell, and herdr types whatever is sent to the terminal into it: a
//! sub-agent's report, for one. Such text is lost, and it can leave the
//! shell waiting for the end of a quote, where the launch would be swallowed
//! too. So an agent whose sub-agents are still working is left alone unless
//! forced, the prompt is cleared before the launch is typed, and the launch
//! is typed again when it did not run.
//! Herdr is driven through its own command, `hc`.
//!
//! There are two kinds of terminal. In one a shell started the agent, and the
//! shell is there again when the agent leaves on Ctrl+C. In the other the
//! agent is the terminal's own program: when it exits, the terminal ends. But
//! herdr puts a shell in place of a kept terminal's program when that program
//! is killed, so there cswap turns keeping on and kills the agent.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::cli::{self, Ctx};
use crate::harness::Harness;
use crate::sessions::{self, Live, Talk};
use crate::store::{Account, Registry};
use crate::{manage, platform, view};

/// Programs that leave a prompt behind when the agent they ran ends.
const SHELLS: [&str; 9] = [
    "zsh", "bash", "fish", "sh", "dash", "ksh", "nu", "pwsh", "xonsh",
];

/// Statuses in which an agent is in the middle of a turn.
const BUSY: [&str; 2] = ["working", "blocked"];

/// Options of the old launch that the new one keeps, and whether each takes
/// a value: the model and the like, so the agent comes back as it was. Herdr
/// starts Codex with `--no-daemon`, because a shared daemon would tie
/// terminals together.
fn carried(h: Harness) -> &'static [(&'static str, bool)] {
    match h {
        Harness::Claude => &[
            ("--model", true),
            ("--effort", true),
            ("--permission-mode", true),
            ("--dangerously-skip-permissions", false),
        ],
        Harness::Codex => &[
            ("--no-daemon", false),
            ("-m", true),
            ("--model", true),
            ("-c", true),
            ("--config", true),
            ("-p", true),
            ("--profile", true),
            ("-s", true),
            ("--sandbox", true),
            ("-a", true),
            ("--ask-for-approval", true),
            ("--full-auto", false),
            ("--dangerously-bypass-approvals-and-sandbox", false),
        ],
        Harness::Grok => &[("-m", true), ("--model", true)],
        Harness::Agy => &[
            ("--model", true),
            ("--effort", true),
            ("--mode", true),
            ("--agent", true),
        ],
    }
}

/// The carried options among the arguments of the old launch, in their order.
/// Only a whole argument counts, so a task that mentions `--model` does not.
fn kept_options(h: Harness, old_args: &[String]) -> Vec<String> {
    let known = carried(h);
    let mut out = Vec::new();
    let mut args = old_args.iter().skip(1);
    while let Some(arg) = args.next() {
        let Some((_, takes_value)) = known
            .iter()
            .find(|(flag, v)| arg == flag || (*v && arg.starts_with(&format!("{flag}="))))
        else {
            continue;
        };
        out.push(arg.clone());
        if *takes_value && !arg.contains('=') {
            match args.next() {
                Some(value) => out.push(value.clone()),
                None => {
                    out.pop();
                }
            }
        }
    }
    out
}

/// Run `hc` and return what it printed. `CSWAP_HC` names another program,
/// which is what the tests use.
pub(crate) fn hc(args: &[&str]) -> Result<String> {
    let bin = std::env::var_os("CSWAP_HC").unwrap_or_else(|| "hc".into());
    let out = Command::new(&bin)
        .args(args)
        // With no sender, a send is typed as it is, without an `<hc>` tag.
        .env_remove("HC_TERMINAL_ID")
        .output()
        .context("herdr's `hc` could not be run: is herdr installed?")?;
    if !out.status.success() {
        bail!(
            "hc {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// What herdr says about one terminal.
struct Terminal {
    address: String,
    live: bool,
    /// `working`, `idle`, `blocked`, `done`; none for a plain program.
    status: Option<String>,
    /// The program in front, which is the agent while one runs.
    front: Option<u32>,
    /// Is the program in front a shell, waiting at its prompt?
    front_shell: bool,
    /// Is the terminal's own program a shell, with the agent started in it?
    shell: bool,
    /// Does herdr put a shell in place of the program when it is killed?
    keep: bool,
}

impl Terminal {
    fn get(t: &str) -> Result<Terminal> {
        let v: Value = serde_json::from_str(&hc(&["get", t, "--json"])?)
            .context("hc get did not print JSON")?;
        let name = |path: &str| {
            Path::new(path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        };
        let program = name(v["argv"][0].as_str().unwrap_or_default());
        let front = name(v["foreground"]["name"].as_str().unwrap_or_default());
        Ok(Terminal {
            address: v["address"].as_str().unwrap_or(t).to_string(),
            live: v["state"] == "live",
            status: v["status"]["state"].as_str().map(str::to_string),
            front: v["foreground"]["pid"]
                .as_u64()
                .and_then(|p| u32::try_from(p).ok()),
            front_shell: SHELLS.contains(&front.as_str()),
            shell: SHELLS.contains(&program.as_str()),
            keep: v["keep"] == true,
        })
    }

    fn busy(&self) -> bool {
        self.status.as_deref().is_some_and(|s| BUSY.contains(&s))
    }
}

/// The agent in a terminal: its account and its conversation.
struct Running<'a> {
    terminal: Terminal,
    acc: &'a Account,
    pid: u32,
    talk: Talk,
    /// The terminals under this one, at any depth, whose agent is in the
    /// middle of a turn: they report to this agent when they are done.
    working_under: Vec<String>,
}

/// One terminal as `hc ls` lists it: its address, the terminal it was
/// started from, and its agent's status.
type Listed = (String, Option<String>, Option<String>);

/// Every live terminal herdr has.
fn listed() -> Result<Vec<Listed>> {
    let v: Value =
        serde_json::from_str(&hc(&["ls", "--json"])?).context("hc ls did not print JSON")?;
    let text = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    Ok(v["terminals"]
        .as_array()
        .context("hc ls listed no terminals")?
        .iter()
        .filter_map(|t| Some((text(&t["address"])?, text(&t["parent"]), text(&t["status"]))))
        .collect())
}

/// The terminals under `t`, children and theirs, that are in a turn.
fn working_under(all: &[Listed], t: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut todo = vec![t.to_string()];
    let mut seen = vec![t.to_string()];
    while let Some(at) = todo.pop() {
        for (address, parent, status) in all {
            if parent.as_deref() != Some(at.as_str()) || seen.contains(address) {
                continue;
            }
            seen.push(address.clone());
            todo.push(address.clone());
            if status.as_deref().is_some_and(|s| BUSY.contains(&s)) {
                out.push(address.clone());
            }
        }
    }
    out.sort();
    out
}

/// The account whose agent is `pid`, or a parent of `pid`: a tool the agent
/// runs can be the program in front for a while.
fn agent_of(c: &Ctx, pid: u32) -> Option<(&Account, u32)> {
    agent_in(&c.reg, &c.live, pid)
}

/// `agent_of` for a caller that has not read everything a command reads.
fn agent_in<'a>(reg: &'a Registry, live: &Live, pid: u32) -> Option<(&'a Account, u32)> {
    let mut at = pid;
    for _ in 0..8 {
        let hit = live
            .iter()
            .find(|(_, pids)| pids.contains(&at))
            .and_then(|(key, _)| reg.get(key));
        if let Some(acc) = hit {
            return Some((acc, at));
        }
        at = sessions::parent(at)?;
    }
    None
}

fn running<'a>(c: &'a Ctx, t: &str) -> Result<Running<'a>> {
    let terminal = Terminal::get(t)?;
    let name = terminal.address.clone();
    if !terminal.live {
        bail!("{name} has ended");
    }
    let (acc, pid) = terminal
        .front
        .and_then(|pid| agent_of(c, pid))
        .with_context(|| format!("{name} is not running an agent that cswap knows"))?;
    let talk = sessions::talk(acc.harness, pid, &acc.config_dir());
    let working_under = working_under(&listed()?, &terminal.address);
    Ok(Running {
        terminal,
        acc,
        pid,
        talk,
        working_under,
    })
}

/// Why this agent cannot be moved now, if it cannot.
fn why_not(r: &Running) -> Option<String> {
    let t = &r.terminal.address;
    if r.terminal.busy() {
        Some(format!(
            "{t} is in the middle of a turn. Try again when it stops."
        ))
    } else if !r.terminal.shell && !cfg!(unix) {
        Some(format!(
            "{t} has no shell behind its agent, and this system cannot put one there."
        ))
    } else {
        None
    }
}

/// Usage in a few words: `week 76% · 5h 12%`.
fn usage_note(c: &Ctx, acc: &Account) -> String {
    if !c.signed_in(acc) {
        return "not signed in".to_string();
    }
    match acc.snapshot() {
        Some(snap) => view::all_windows(snap)
            .iter()
            .map(|w| format!("{} {:.0}%", w.label, w.used_now(c.now)))
            .collect::<Vec<_>>()
            .join(" · "),
        None => String::new(),
    }
}

/// One account as a menu row. `current` is the key of the account in use.
fn row(c: &Ctx, acc: &Account, current: Option<&str>) -> Value {
    let mut v = cli::account_json(c, acc);
    v["root"] = json!(acc.is_native());
    // What `--cswap=` takes to start on this account.
    v["selector"] = json!(selector(acc));
    v["usage"] = json!(usage_note(c, acc));
    v["current"] = json!(current == Some(acc.key().as_str()));
    v
}

/// The root login, then the accounts.
fn rows(c: &Ctx, h: Harness, current: Option<&str>) -> Vec<Value> {
    let (roots, rest): (Vec<&Account>, Vec<&Account>) = c.reg.of(h).partition(|a| a.is_native());
    roots
        .into_iter()
        .chain(rest)
        .map(|a| row(c, a, current))
        .collect()
}

/// How `--cswap=` names an account in a line a person will read: its name
/// when that is one plain word, its key otherwise.
fn selector(acc: &Account) -> String {
    let plain = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    if acc.is_native() {
        "root".to_string()
    } else if plain(&acc.meta.label) {
        acc.meta.label.clone()
    } else {
        acc.key()
    }
}

fn quote(arg: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "-_=./:@,+".contains(c);
    if !arg.is_empty() && arg.chars().all(safe) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// The line that starts `to`'s agent again, typed at the shell.
fn launch_line(to: &Account, old_args: &[String], resume: &[String]) -> String {
    let h = to.harness;
    let mut words = vec![
        "cswap".to_string(),
        h.id().to_string(),
        format!("--cswap={}", selector(to)),
    ];
    words.extend(kept_options(h, old_args));
    words.extend(resume.iter().cloned());
    words.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ")
}

fn gone_within(pid: u32, wait: Duration) -> bool {
    let until = Instant::now() + wait;
    loop {
        if !platform::pid_alive(pid) {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// Stop an agent that a shell started, the way a person would. An agent
/// leaves cleanly on Ctrl+C: once when it is idle, more with a draft or a
/// dialog open. The presses come quicker than an agent forgets the one
/// before. A signal is the last resort, because some agents leave the
/// terminal in their own mode when killed.
fn stop(t: &str, pid: u32) -> Result<()> {
    for _ in 0..6 {
        hc(&["keys", t, "ctrl+c"])?;
        if gone_within(pid, Duration::from_millis(350)) {
            return Ok(());
        }
    }
    if gone_within(pid, Duration::from_secs(2)) {
        return Ok(());
    }
    platform::terminate(pid, false);
    if gone_within(pid, Duration::from_secs(3)) {
        return Ok(());
    }
    bail!("the agent in {t} (pid {pid}) did not stop; nothing else was changed")
}

/// Stop an agent that is the terminal's own program. A clean exit would end
/// the terminal, and its place in the window with it. Herdr puts a shell in
/// place of a kept terminal's program only when that program is killed, so
/// keeping is turned on and the agent is killed. It is idle, and an agent
/// writes its conversation down as it goes, so nothing said is lost.
fn replace_with_shell(term: &Terminal, pid: u32) -> Result<()> {
    let t = &term.address;
    if !term.keep {
        hc(&["keep", t])?;
    }
    platform::terminate(pid, true);
    if !gone_within(pid, Duration::from_secs(3)) {
        bail!("the agent in {t} (pid {pid}) did not stop; nothing else was changed");
    }
    Ok(())
}

/// Wait until `seen` says yes about the terminal, for at most `wait`.
fn wait_for(t: &str, wait: Duration, seen: impl Fn(&Terminal) -> Result<bool>) -> Result<bool> {
    let until = Instant::now() + wait;
    loop {
        if seen(&Terminal::get(t)?)? {
            return Ok(true);
        }
        if Instant::now() >= until {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

/// What a switch or a restart did.
pub struct Switched {
    pub terminal: String,
    pub agent: Harness,
    pub from: String,
    pub to: String,
    /// The conversation it went on with; none when it started a new one.
    pub session: Option<String>,
    /// Was the agent seen running on the new account?
    pub started: bool,
    pub line: String,
}

/// What the caller allows a switch or a restart to do.
#[derive(Debug, Clone, Copy, Default)]
pub struct Allow {
    /// A new conversation, when the running one cannot be told.
    pub fresh: bool,
    /// Going ahead while sub-agents are still working.
    pub force: bool,
}

/// Move the agent in terminal `t` to the account `to`.
pub fn switch(c: &Ctx, t: &str, to: &str, allow: Allow) -> Result<Switched> {
    let r = running(c, t)?;
    let target = c.reg.find(r.acc.harness, to)?;
    if target.key() == r.acc.key() {
        bail!(
            "{} already runs on {}. To start it again there: cswap herdr restart {}",
            r.terminal.address,
            target.title(),
            r.terminal.address
        );
    }
    relaunch(c, r, target, allow)
}

/// Stop the agent in terminal `t` and start it again on the account it is on,
/// in the same conversation. The new one is the program on disk now, which is
/// how an update to the agent, to cswap or to the agent's settings takes hold.
pub fn restart(c: &Ctx, t: &str, allow: Allow) -> Result<Switched> {
    let r = running(c, t)?;
    let target = r.acc;
    relaunch(c, r, target, allow)
}

/// Why an agent with sub-agents at work is left alone.
fn children_note(t: &str, working: &[String]) -> String {
    format!(
        "{t} has {} still working: {}. A report one of them sends while {t} is stopped \
         is typed into its shell and lost.",
        if working.len() == 1 {
            "a sub-agent".to_string()
        } else {
            format!("{} sub-agents", working.len())
        },
        working.join(", ")
    )
}

/// Type the launch at the shell in `t` and wait for the agent to run on
/// `key`. Text that reached the shell first, such as a report meant for the
/// agent, can leave it waiting for the end of a quote, and a line typed there
/// never runs. So the prompt is cleared before the line is typed when the
/// shell was there all along (`clear`), and when the shell is still in front
/// a while after the line, the prompt is cleared and the line typed again.
/// A new shell is not cleared at first: a Ctrl+C would stop it reading its
/// own rc file. False when the agent is not seen running in the end.
fn launch(c: &Ctx, t: &str, line: &str, key: &str, clear: bool) -> Result<bool> {
    // How long a shell may stay in front after the line before the line
    // counts as not run. A shell that was there runs it at once; one that
    // herdr has just started may still be loading.
    let settle = Duration::from_millis(if clear { 2500 } else { 8000 });
    for attempt in 0..3 {
        if clear || attempt > 0 {
            hc(&["keys", t, "ctrl+c"])?;
        }
        hc(&["send", t, line])?;
        let typed = Instant::now();
        let not_run = std::cell::Cell::new(false);
        let started = wait_for(t, Duration::from_secs(8), |term| {
            let live = sessions::scan(&c.reg);
            let runs = term
                .front
                .is_some_and(|pid| live.get(key).is_some_and(|pids| pids.contains(&pid)));
            not_run.set(!runs && term.front_shell && typed.elapsed() >= settle);
            Ok(runs || not_run.get())
        })?;
        if started && !not_run.get() {
            return Ok(true);
        }
        // Something other than a shell is in front: the line ran, and what
        // it started is not to be interrupted.
        if !Terminal::get(t)?.front_shell {
            return Ok(false);
        }
    }
    Ok(false)
}

/// Stop the agent `r` and start it on `target` with its conversation.
fn relaunch(c: &Ctx, r: Running, target: &Account, allow: Allow) -> Result<Switched> {
    let t = r.terminal.address.clone();
    let h = r.acc.harness;
    if !c.signed_in(target) {
        bail!(
            "{} is not signed in: run `cswap login {}` first",
            target.title(),
            target.key()
        );
    }
    if let Some(why) = why_not(&r) {
        bail!(why);
    }
    if !allow.force && !r.working_under.is_empty() {
        bail!(
            "{} Wait for them, or go ahead with `--force` and check on them afterwards.",
            children_note(&t, &r.working_under)
        );
    }
    let (session, resume) = match &r.talk {
        Talk::Open { id, resume } => (Some(id.clone()), resume.clone()),
        Talk::Fresh => (None, Vec::new()),
        Talk::Unknown if allow.fresh => (None, Vec::new()),
        Talk::Unknown => bail!(
            "cswap cannot tell which conversation {t} is in, so it was left alone. \
             `--fresh` goes ahead anyway and starts a new one."
        ),
    };
    let line = launch_line(target, &sessions::command_line(r.pid), &resume);

    // A shell takes the front; herdr notices a moment later.
    let old = r.pid;
    let back = if r.terminal.shell {
        stop(&t, old)?;
        wait_for(&t, Duration::from_secs(4), |term| {
            if !term.live {
                bail!("{t} ended with its agent. Start it again with: {line}");
            }
            Ok(term.front != Some(old))
        })?
    } else {
        replace_with_shell(&r.terminal, old)?;
        wait_for(&t, Duration::from_secs(6), |term| {
            Ok(term.live && term.shell && term.front != Some(old))
        })?
    };
    if !back {
        bail!("the agent in {t} stopped but no shell came back. Start it again with: {line}");
    }
    let started = launch(c, &t, &line, &target.key(), r.terminal.shell)?;
    Ok(Switched {
        terminal: t,
        agent: h,
        from: r.acc.title(),
        to: target.title(),
        session,
        started,
        line,
    })
}

// ------------------------------------------------------------------- commands

/// `cswap herdr menu`: every installed agent with its accounts, as JSON.
pub fn menu_cmd() -> Result<()> {
    let c = Ctx::load()?;
    let agents: Vec<Value> = Harness::ALL
        .into_iter()
        .filter(|h| manage::installed(*h))
        .map(|h| {
            json!({
                "agent": h.id(),
                "name": h.name(),
                "bins": h.spec().bins,
                // `auto`, or the key of the account a plain launch uses.
                "start": match c.hs[&h].start.as_str() {
                    "auto" => "auto".to_string(),
                    _ => c.reg.of(h).find(|a| c.is_default(a)).map(Account::key).unwrap_or_default(),
                },
                "accounts": rows(&c, h, None),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&agents)?);
    Ok(())
}

/// The agent in each herdr terminal and its account. Herdr tells a program
/// its terminal in `HC_TERMINAL_ID`, so nothing is asked of herdr here. An
/// agent that another agent started in the same terminal is left out: the
/// terminal's agent is the outer one.
fn in_terminals(c: &Ctx) -> Vec<(String, u32, &Account)> {
    let mut found: Vec<(String, u32, &Account)> = Vec::new();
    for acc in &c.reg.accounts {
        for pid in c.live.get(&acc.key()).into_iter().flatten() {
            if let Some(terminal) = sessions::env_of(*pid, "HC_TERMINAL_ID") {
                found.push((terminal, *pid, acc));
            }
        }
    }
    let inner = |terminal: &str, pid: u32| {
        let mut at = pid;
        for _ in 0..8 {
            match sessions::parent(at) {
                Some(up) if found.iter().any(|(t, p, _)| t == terminal && *p == up) => return true,
                Some(up) => at = up,
                None => break,
            }
        }
        false
    };
    let mut outer: Vec<_> = found
        .iter()
        .filter(|(t, pid, _)| !inner(t, *pid))
        .cloned()
        .collect();
    outer.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    outer
}

/// `cswap herdr ls`: which account each herdr terminal's agent runs on.
pub fn ls_cmd(as_json: bool) -> Result<()> {
    let c = Ctx::load()?;
    let rows = in_terminals(&c);
    if as_json {
        let all: Vec<Value> = rows
            .iter()
            .map(|(terminal, pid, acc)| {
                json!({
                    "terminal": terminal,
                    "agent": acc.harness.id(),
                    "account": acc.key(),
                    "name": acc.title(),
                    "pid": pid,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("No agent is running in a herdr terminal.");
        return Ok(());
    }
    for (terminal, _, acc) in &rows {
        println!(
            "{}  {} on {}",
            c.p.bold(&format!("{terminal:<6}")),
            acc.harness.name(),
            c.p.bold(&acc.title())
        );
    }
    Ok(())
}

/// `cswap herdr info <terminal>`: the account in use and the ones on offer.
pub fn info_cmd(t: &str, as_json: bool) -> Result<()> {
    let c = Ctx::load()?;
    let r = running(&c, t)?;
    let h = r.acc.harness;
    let key = r.acc.key();
    if as_json {
        let session = match &r.talk {
            Talk::Open { id, .. } => Some(id.as_str()),
            _ => None,
        };
        let v = json!({
            "terminal": r.terminal.address,
            "agent": h.id(),
            "agent_name": h.name(),
            "pid": r.pid,
            "account": key,
            "name": r.acc.title(),
            "status": r.terminal.status,
            "conversation": r.talk.word(),
            "session": session,
            "why_not": why_not(&r),
            // Sub-agents in a turn: a switch or a restart needs `--force`.
            "children_working": r.working_under,
            "accounts": rows(&c, h, Some(&key)),
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    println!(
        "{}  {} on {} ({key})",
        c.p.bold(&r.terminal.address),
        h.name(),
        c.p.bold(&r.acc.title())
    );
    let talk = match &r.talk {
        Talk::Open { id, .. } => format!("conversation {id}"),
        Talk::Fresh => "nothing said yet".to_string(),
        Talk::Unknown => "conversation not found: a switch or a restart needs --fresh".to_string(),
    };
    println!("{}", c.p.dim(&talk));
    if let Some(why) = why_not(&r) {
        println!("{}", c.p.warn(&why));
    }
    if !r.working_under.is_empty() {
        println!(
            "{}",
            c.p.warn(&format!(
                "{} A switch or a restart needs --force.",
                children_note(&r.terminal.address, &r.working_under)
            ))
        );
    }
    Ok(())
}

/// `cswap herdr switch <terminal> <account>`.
pub fn switch_cmd(t: &str, to: &str, allow: Allow, as_json: bool) -> Result<()> {
    let c = Ctx::load()?;
    let s = switch(&c, t, to, allow)?;
    if as_json {
        let v = json!({
            "terminal": s.terminal,
            "agent": s.agent.id(),
            "from": s.from,
            "to": s.to,
            "session": s.session,
            "resumed": s.session.is_some(),
            "started": s.started,
            "line": s.line,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let how = if s.session.is_some() {
        "same conversation"
    } else {
        "new conversation"
    };
    println!(
        "{}  {} moved from {} to {}, {how}.",
        c.p.bold(&s.terminal),
        s.agent.name(),
        s.from,
        c.p.bold(&s.to)
    );
    if !s.started {
        println!(
            "{}",
            c.p.warn("The launch was typed, but the agent is not seen running yet: look at the terminal.")
        );
    }
    Ok(())
}

/// `cswap herdr restart <terminal>`.
pub fn restart_cmd(t: &str, allow: Allow, as_json: bool) -> Result<()> {
    let c = Ctx::load()?;
    let s = restart(&c, t, allow)?;
    if as_json {
        let v = json!({
            "terminal": s.terminal,
            "agent": s.agent.id(),
            "account": s.to,
            "session": s.session,
            "resumed": s.session.is_some(),
            "started": s.started,
            "line": s.line,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let how = if s.session.is_some() {
        "same conversation"
    } else {
        "new conversation"
    };
    println!(
        "{}  {} started again on {}, {how}.",
        c.p.bold(&s.terminal),
        s.agent.name(),
        c.p.bold(&s.to)
    );
    if !s.started {
        println!(
            "{}",
            c.p.warn("The launch was typed, but the agent is not seen running yet: look at the terminal.")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Sandbox;

    #[test]
    fn the_launch_line_names_the_account_keeps_herdrs_flag_and_resumes() {
        let sb = Sandbox::new();
        sb.demo();
        let c = Ctx::load().unwrap();
        let codex = c.reg.of(Harness::Codex).find(|a| !a.is_native()).unwrap();
        // The options come along; the old conversation and the task do not.
        let old = [
            "codex",
            "--no-daemon",
            "-m",
            "some-model",
            "-c",
            "model_reasoning_effort=\"high\"",
            "resume",
            "01a1c441-6f3c-7d70-8853-ed8d54d0fb51",
            "A task that says --model in passing",
        ]
        .map(String::from);
        let resume = ["resume", "01a18f0e-3563-7043-886f-5dd31ed516a2"].map(String::from);
        assert_eq!(
            launch_line(codex, &old, &resume),
            format!(
                "cswap codex --cswap={} --no-daemon -m some-model -c 'model_reasoning_effort=\"high\"' resume 01a18f0e-3563-7043-886f-5dd31ed516a2",
                selector(codex)
            )
        );
        let claude = [
            "claude",
            "--model=opus",
            "--effort",
            "high",
            "--continue",
            "go",
        ]
        .map(String::from);
        assert_eq!(
            kept_options(Harness::Claude, &claude),
            ["--model=opus", "--effort", "high"]
        );
        // An option whose value is missing is dropped, not passed half.
        let cut = ["claude", "--model"].map(String::from);
        assert!(kept_options(Harness::Claude, &cut).is_empty());
        // A launch that had no such flag gets none, and a fresh start no resume.
        let root = c.reg.of(Harness::Claude).find(|a| a.is_native()).unwrap();
        assert_eq!(
            launch_line(root, &["claude".to_string()], &[]),
            "cswap claude --cswap=root"
        );
    }

    #[test]
    fn sub_agents_at_work_are_found_at_any_depth() {
        let t = |a: &str, parent: Option<&str>, status: Option<&str>| {
            (
                a.to_string(),
                parent.map(str::to_string),
                status.map(str::to_string),
            )
        };
        let all = [
            t("t1", None, Some("idle")),
            t("t2", Some("t1"), Some("done")),
            t("t3", Some("t1"), Some("working")),
            // Under a child that is itself done.
            t("t4", Some("t2"), Some("blocked")),
            // A plain program has no status.
            t("t5", Some("t1"), None),
            // Someone else's.
            t("t6", Some("t9"), Some("working")),
            t("t9", None, Some("idle")),
        ];
        assert_eq!(working_under(&all, "t1"), ["t3", "t4"]);
        assert_eq!(working_under(&all, "t2"), ["t4"]);
        assert!(working_under(&all, "t3").is_empty());
        assert_eq!(working_under(&all, "t9"), ["t6"]);
    }

    #[test]
    fn a_word_that_a_shell_would_read_is_quoted() {
        assert_eq!(quote("--cswap=work"), "--cswap=work");
        assert_eq!(quote("two words"), "'two words'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
    }
}
