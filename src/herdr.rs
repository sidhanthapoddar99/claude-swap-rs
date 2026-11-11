//! Herdr keeps agents in terminals that stay alive. This is what cswap does
//! for it: say which account the agent in a terminal runs on, list what a
//! launch menu can offer, and move a running agent to another account without
//! losing its conversation.
//!
//! A switch stops the agent, waits for a shell in its place, and types the
//! launch on the new account with the arguments that resume the conversation.
//! The terminal, its tab, its place and its sub-agents stay as they are.
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
    /// Is the terminal's own program a shell, with the agent started in it?
    shell: bool,
    /// Does herdr put a shell in place of the program when it is killed?
    keep: bool,
    /// The terminal it was started from, when it has one.
    parent: Option<String>,
}

impl Terminal {
    fn get(t: &str) -> Result<Terminal> {
        let v: Value = serde_json::from_str(&hc(&["get", t, "--json"])?)
            .context("hc get did not print JSON")?;
        let first = v["argv"][0].as_str().unwrap_or_default();
        let program = Path::new(first)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        Ok(Terminal {
            address: v["address"].as_str().unwrap_or(t).to_string(),
            live: v["state"] == "live",
            status: v["status"]["state"].as_str().map(str::to_string),
            front: v["foreground"]["pid"]
                .as_u64()
                .and_then(|p| u32::try_from(p).ok()),
            shell: SHELLS.contains(&program.as_str()),
            keep: v["keep"] == true,
            parent: v["parent"]
                .as_str()
                .filter(|p| !p.is_empty())
                .map(str::to_string),
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

/// The account a sub-agent takes from the agent above it.
///
/// herdr starts every terminal in the login environment, not in the one of
/// the agent that asked for it, so the account that agent runs on is not
/// passed down the way it is outside herdr. This looks it up instead: the
/// nearest terminal above this one that runs an agent of the same kind.
/// `None` outside herdr, at the top of a tree, and whenever herdr cannot be
/// asked: a launch never fails over this.
pub fn parent_account(reg: &Registry, h: Harness) -> Option<String> {
    let me = std::env::var("HC_TERMINAL_ID")
        .ok()
        .filter(|v| !v.trim().is_empty())?;
    let mut above = Terminal::get(&me).ok()?.parent?;
    let live = sessions::scan(reg);
    for _ in 0..6 {
        let t = Terminal::get(&above).ok()?;
        let agent = t
            .front
            .filter(|_| t.live)
            .and_then(|pid| agent_in(reg, &live, pid));
        match agent {
            Some((acc, _)) if acc.harness == h => return Some(acc.key()),
            _ => above = t.parent?,
        }
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
    Ok(Running {
        terminal,
        acc,
        pid,
        talk,
    })
}

/// Why this agent cannot be moved now, if it cannot.
fn why_not(r: &Running) -> Option<String> {
    let t = &r.terminal.address;
    if r.terminal.busy() {
        Some(format!(
            "{t} is in the middle of a turn. Switch when it stops."
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

/// What a switch did.
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

/// Move the agent in terminal `t` to the account `to`. `fresh` allows a new
/// conversation when the running one cannot be told.
pub fn switch(c: &Ctx, t: &str, to: &str, fresh: bool) -> Result<Switched> {
    let r = running(c, t)?;
    let t = r.terminal.address.clone();
    let h = r.acc.harness;
    let target = c.reg.find(h, to)?;
    if target.key() == r.acc.key() {
        bail!("{t} already runs on {}", target.title());
    }
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
    let (session, resume) = match &r.talk {
        Talk::Open { id, resume } => (Some(id.clone()), resume.clone()),
        Talk::Fresh => (None, Vec::new()),
        Talk::Unknown if fresh => (None, Vec::new()),
        Talk::Unknown => bail!(
            "cswap cannot tell which conversation {t} is in, so it was left alone. \
             `--fresh` switches anyway and starts a new one."
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
    hc(&["send", &t, &line])?;

    let key = target.key();
    let started = wait_for(&t, Duration::from_secs(8), |term| {
        let live = sessions::scan(&c.reg);
        Ok(term
            .front
            .is_some_and(|pid| live.get(&key).is_some_and(|pids| pids.contains(&pid))))
    })?;
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
        Talk::Unknown => "conversation not found: a switch needs --fresh".to_string(),
    };
    println!("{}", c.p.dim(&talk));
    if let Some(why) = why_not(&r) {
        println!("{}", c.p.warn(&why));
    }
    Ok(())
}

/// `cswap herdr switch <terminal> <account>`.
pub fn switch_cmd(t: &str, to: &str, fresh: bool, as_json: bool) -> Result<()> {
    let c = Ctx::load()?;
    let s = switch(&c, t, to, fresh)?;
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
    fn a_word_that_a_shell_would_read_is_quoted() {
        assert_eq!(quote("--cswap=work"), "--cswap=work");
        assert_eq!(quote("two words"), "'two words'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
    }
}
