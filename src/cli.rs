//! The plain commands: everything `cswap <command>` prints line by line.
//!
//! They read the same files the dashboard does. Only `refresh` (and `usage
//! --refresh`) reach the network; the rest work offline from what was last
//! stored.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};

use crate::harness::{Harness, Local};
use crate::health::{self, Level};
use crate::sessions::{self, Live};
use crate::store::{Account, HarnessSettings, Registry, Settings};
use crate::sync::{self, State};
use crate::text::{fit, fit_right, Paint};
use crate::view::{self, Kind};
use crate::{manage, model, paths, picker, refresh, shell, suggest, wrap};

/// Everything a command needs, read once.
pub struct Ctx {
    pub reg: Registry,
    pub settings: Settings,
    pub hs: HashMap<Harness, HarnessSettings>,
    pub logged_in: HashSet<String>,
    pub local: HashMap<String, Local>,
    pub live: Live,
    pub now: DateTime<Utc>,
    pub p: Paint,
}

impl Ctx {
    pub fn load() -> Result<Ctx> {
        let mut reg = Registry::load()?;
        let (logged_in, local) = view::facts(&mut reg);
        Ok(Ctx {
            live: sessions::scan(&reg),
            settings: Settings::load()?,
            hs: Harness::ALL
                .into_iter()
                .map(|h| Ok((h, HarnessSettings::load(h)?)))
                .collect::<Result<_>>()?,
            logged_in,
            local,
            reg,
            now: Utc::now(),
            p: Paint::stdout(),
        })
    }

    pub fn signed_in(&self, acc: &Account) -> bool {
        self.logged_in.contains(&acc.key())
    }

    pub fn is_default(&self, acc: &Account) -> bool {
        view::is_default(acc, &self.hs[&acc.harness])
    }

    fn state(&self, acc: &Account) -> view::State {
        view::state(acc, self.signed_in(acc), false, self.now)
    }

    fn state_text(&self, acc: &Account) -> String {
        let s = self.state(acc);
        let text = s.label();
        match s {
            view::State::Ready => self.p.ok(&text),
            view::State::Blocked => self.p.bad(&text),
            view::State::Stale => self.p.warn(&text),
            _ => self.p.dim(&text),
        }
    }

    /// The agents that have at least one cswap account, in their fixed order.
    fn agents(&self) -> Vec<Harness> {
        Harness::ALL
            .into_iter()
            .filter(|h| self.reg.profiles().any(|a| a.harness == *h))
            .collect()
    }

    /// The cswap account that holds the same login as `acc`, an agent's
    /// default.
    fn twin(&self, acc: &Account) -> Option<&Account> {
        manage::duplicate_of(&self.reg, acc)
    }

    fn agent_header(&self, h: Harness) -> String {
        let hs = &self.hs[&h];
        let default = match hs.start.as_str() {
            "root" => "its root login".to_string(),
            "auto" => "the best account (auto)".to_string(),
            id => self
                .reg
                .of(h)
                .find(|a| a.meta.id.to_string() == id)
                .map(|a| a.title())
                .unwrap_or_else(|| format!("{id} (missing)")),
        };
        let mut notes = vec![format!("`{}` starts on {default}", h.spec().bins[0])];
        if !manage::installed(h) {
            notes.push("not installed".to_string());
        }
        if h.spec().experimental {
            notes.push("experimental".to_string());
        }
        format!(
            "{}  {}",
            self.p.bold(h.name()),
            self.p.dim(&notes.join(" · "))
        )
    }
}

pub fn harness(text: &str) -> Result<Harness> {
    Harness::from_id(text)
        .with_context(|| format!("'{text}' is not an agent. Use one of: claude, codex, agy, grok"))
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn id_text(acc: &Account) -> String {
    if acc.is_native() {
        "0".to_string()
    } else {
        acc.meta.id.to_string()
    }
}

// ----------------------------------------------------------------------- list

pub fn account_json(c: &Ctx, acc: &Account) -> serde_json::Value {
    let snap = acc.snapshot();
    json!({
        "key": acc.key(),
        "agent": acc.harness.id(),
        "id": acc.meta.id,
        "name": acc.title(),
        "aliases": acc.meta.aliases,
        "email": acc.meta.identity.email,
        "plan": view::plan_text(acc, &c.local),
        "starts_here": c.is_default(acc),
        "signed_in": c.signed_in(acc),
        "state": c.state(acc).label(),
        "fetched_at": snap.map(|s| s.fetched_at),
        "windows": snap.map(|s| view::usage_windows(s).into_iter().map(|w| json!({
            "label": w.label,
            "used_pct": w.used_now(c.now),
            "resets_at": w.resets_at,
            "window_secs": w.window_secs,
        })).collect::<Vec<_>>()).unwrap_or_default(),
        "spare_resets": acc.grants(c.now).iter().map(|g| json!({
            "scope": g.scope,
            "count": g.count,
            "expires_at": g.expires_at,
        })).collect::<Vec<_>>(),
        "renews_on": acc.meta.billing.next(crate::store::today()),
        "launch": view::launch_hint(acc, c.is_default(acc)),
        "error": acc.meta.refresh.error,
    })
}

/// The usage of one account in a single cell: bar, percent and countdown.
fn usage_cell(c: &Ctx, acc: &Account) -> String {
    let p = &c.p;
    match acc
        .snapshot()
        .and_then(view::main_window)
        .filter(|_| c.signed_in(acc))
    {
        Some(w) => {
            let used = w.used_now(c.now);
            let reset = match w.resets_at {
                Some(at) if at > c.now => format!("in {}", model::until(at, c.now)),
                Some(_) => "has reset".to_string(),
                None => String::new(),
            };
            format!(
                "{} {}  {}",
                p.bar(used, 12),
                p.severity(used, &format!("{used:>3.0}%")),
                p.dim(&fit(&reset, 12))
            )
        }
        None => {
            let text = if !c.signed_in(acc) {
                "not signed in".to_string()
            } else {
                match &acc.meta.refresh.error {
                    Some(e) => e.clone(),
                    None => "no usage yet: cswap refresh".to_string(),
                }
            };
            p.dim(&fit(&text, 31))
        }
    }
}

const NO_ACCOUNTS: &str = "No accounts yet. Add one with: cswap add <agent> <name>\n\
     Your agents' own logins are listed by: cswap defaults";

pub fn list(as_json: bool) -> Result<()> {
    let c = Ctx::load()?;
    if as_json {
        let all: Vec<_> = c.reg.profiles().map(|a| account_json(&c, a)).collect();
        println!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(());
    }
    if c.reg.profiles().next().is_none() {
        println!("{NO_ACCOUNTS}");
        return Ok(());
    }
    let p = &c.p;
    for (i, h) in c.agents().into_iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{}", c.agent_header(h));
        for acc in c.reg.of(h).filter(|a| !a.is_native()) {
            let star = if c.is_default(acc) {
                p.accent("★")
            } else {
                " ".to_string()
            };
            println!(
                "  {star} {} {} {} {} {}  {}",
                p.dim(&fit_right(&id_text(acc), 2)),
                p.bold(&fit(&acc.title(), 18)),
                p.dim(&fit(acc.email(), 28)),
                fit(&view::plan_text(acc, &c.local), 10),
                usage_cell(&c, acc),
                c.state_text(acc),
            );
        }
    }
    println!();
    println!(
        "{}",
        p.dim("★ the default: where `cswap <agent>` starts · `cswap usage` for every limit · `cswap` alone opens the dashboard")
    );
    Ok(())
}

/// `cswap defaults`: each agent's own login.
pub fn root(as_json: bool) -> Result<()> {
    let c = Ctx::load()?;
    if as_json {
        let all: Vec<_> = c
            .reg
            .roots()
            .map(|a| {
                let mut v = account_json(&c, a);
                v["same_login_as"] = json!(c.twin(a).map(|t| t.title()));
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(());
    }
    if c.reg.roots().next().is_none() {
        println!("No agent found. cswap looks for claude, codex, agy and grok.");
        return Ok(());
    }
    let p = &c.p;
    println!("{}", p.bold("Root logins"));
    println!(
        "  {}",
        p.dim("each agent's own login, the one it had before cswap")
    );
    for acc in c.reg.roots() {
        let h = acc.harness;
        let star = if c.is_default(acc) {
            p.accent("★")
        } else {
            " ".to_string()
        };
        let twin = match c.twin(acc) {
            Some(t) => format!("same login as {}", t.title()),
            None if c.signed_in(acc) => "no account holds this login".to_string(),
            None => String::new(),
        };
        println!(
            "  {star} {} {} {} {}  {}",
            p.bold(&fit(h.name(), 12)),
            p.dim(&fit(acc.email(), 28)),
            fit(&view::plan_text(acc, &c.local), 10),
            usage_cell(&c, acc),
            p.dim(&twin),
        );
    }
    println!();
    println!(
        "{}",
        p.dim("★ the default: where `cswap <agent>` starts · make an account from a login with: cswap add <agent> <name>")
    );
    Ok(())
}

// ---------------------------------------------------------------------- usage

fn fetch(keys: &[String]) -> Vec<(String, Option<String>)> {
    std::thread::scope(|s| {
        let jobs: Vec<_> = keys
            .iter()
            .map(|key| s.spawn(move || (key.clone(), refresh::fetch(key, None).err())))
            .collect();
        jobs.into_iter()
            .map(|j| {
                j.join()
                    .unwrap_or_else(|_| (String::new(), Some("the fetch crashed".into())))
            })
            .collect()
    })
}

/// The keys to act on: one named account, or every signed-in login once. A
/// root login that an account also holds is answered by that account's fetch.
fn targets(c: &Ctx, account: Option<&str>) -> Result<Vec<String>> {
    match account {
        Some(sel) => Ok(vec![c.reg.find_any(sel)?.key()]),
        None => Ok(c
            .reg
            .accounts
            .iter()
            .filter(|a| c.signed_in(a))
            .filter(|a| refresh::carrier(&c.reg.accounts, a).is_none())
            .map(|a| a.key())
            .collect()),
    }
}

pub fn refresh_cmd(account: Option<&str>) -> Result<()> {
    let c = Ctx::load()?;
    let keys = targets(&c, account)?;
    if keys.is_empty() {
        println!("No signed-in account to refresh.");
        return Ok(());
    }
    let results = fetch(&keys);
    let c = Ctx::load()?;
    let p = &c.p;
    let mut failed = 0;
    for (key, error) in results {
        let Some(acc) = c.reg.get(&key) else { continue };
        let name = format!("{} {}", fit(&key, 10), fit(&acc.title(), 18));
        match error {
            None => println!("{} {name} {}", p.ok("✓"), usage_cell(&c, acc)),
            Some(e) => {
                failed += 1;
                println!("{} {name} {}", p.bad("✗"), p.warn(&e));
            }
        }
    }
    if failed > 0 && account.is_some() {
        bail!("the fetch failed");
    }
    Ok(())
}

pub fn usage(account: Option<&str>, refresh_first: bool) -> Result<()> {
    if refresh_first {
        let c = Ctx::load()?;
        fetch(&targets(&c, account)?);
    }
    let c = Ctx::load()?;
    let p = &c.p;
    let accounts: Vec<&Account> = match account {
        Some(sel) => vec![c.reg.find_any(sel)?],
        None => c.reg.profiles().collect(),
    };
    if accounts.is_empty() {
        println!("{NO_ACCOUNTS}");
        return Ok(());
    }
    for (i, acc) in accounts.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let mut title = format!("{} · {}", acc.title(), acc.harness.name());
        let plan = view::plan_text(acc, &c.local);
        if !plan.is_empty() {
            title.push_str(&format!(" · {plan}"));
        }
        let fetched = acc
            .snapshot()
            .map(|s| format!("fetched {}", model::ago(s.fetched_at, c.now)))
            .unwrap_or_default();
        println!(
            "{}  {}  {}",
            p.bold(&title),
            c.state_text(acc),
            p.dim(&fetched)
        );

        let Some(snap) = acc.snapshot().filter(|_| c.signed_in(acc)) else {
            let text = if !c.signed_in(acc) {
                format!("not signed in: cswap login {}", acc.key())
            } else {
                match &acc.meta.refresh.error {
                    Some(e) => e.clone(),
                    None => "no usage yet: cswap refresh".to_string(),
                }
            };
            println!("  {}", p.dim(&text));
            continue;
        };
        // One line per limit, in one grid: name, bar, percent, time left.
        for w in view::usage_windows(snap) {
            let used = w.used_now(c.now);
            let reset = match w.resets_at {
                Some(at) if at > c.now => format!("in {}", model::until(at, c.now)),
                Some(_) => "has reset".to_string(),
                None => String::new(),
            };
            let warning = view::pace_warning(w, c.now).unwrap_or_default();
            println!(
                "  {} {} {}  {} {}",
                fit(&w.label, 12),
                p.bar(used, 24),
                p.severity(used, &format!("{used:>3.0}%")),
                fit(&reset, 10),
                p.warn(&warning)
            );
        }
        if let Some(text) = snap.credits.as_ref().and_then(view::credits_text) {
            println!("  {} {text}", fit("extra usage", 12));
        }
        if let Some(e) = &acc.meta.refresh.error {
            println!("  {} {}", fit("last fetch", 12), p.warn(e));
        }
    }
    Ok(())
}

// --------------------------------------------------------------------- resets

pub fn resets() -> Result<()> {
    let c = Ctx::load()?;
    let p = &c.p;
    if c.reg.profiles().next().is_none() {
        println!("{NO_ACCOUNTS}");
        return Ok(());
    }
    let name = |key: &str| -> String {
        c.reg
            .get(key)
            .map(|a| format!("{} · {}", a.title(), a.harness.name()))
            .unwrap_or_else(|| key.to_string())
    };

    // One table shape for all three parts, so the columns line up. A
    // 12-hour clock needs three more cells for the date.
    let date_w = if c.settings.clock_24h { 18 } else { 21 };
    // Only a spare reset has a count; the column keeps its place in all three.
    let table = |title: &str, count: bool, heads: [&str; 4], rows: &[view::Event], empty: &str| {
        println!("{}", p.bold(title));
        if rows.is_empty() {
            println!("  {}", p.dim(empty));
            return;
        }
        println!(
            "  {}",
            p.dim(&format!(
                "{} {:>5}  {} {} {} {}",
                fit("ACCOUNT", 28),
                if count { "COUNT" } else { "" },
                fit(heads[0], 12),
                fit(heads[1], 10),
                fit(heads[2], date_w),
                heads[3]
            ))
        );
        for e in rows {
            let (when, date) = match e.at {
                Some(_) => (e.countdown(c.now), e.date(c.now, c.settings.clock_24h)),
                None => ("–".to_string(), "does not expire".to_string()),
            };
            let soon = e.kind == Kind::Spare
                && e.at
                    .is_some_and(|at| at - c.now <= chrono::Duration::days(3));
            let ends = e.kind == Kind::PlanEnds;
            let when = fit(&when, 10);
            let what = fit(&e.what, 12);
            // How full the account's week is; for a renewal, its cycle.
            let last = match (e.kind, e.used) {
                (Kind::Renewal, _) => p.dim(&e.note),
                (Kind::PlanEnds, _) => p.warn(&e.note),
                (_, Some(used)) => format!(
                    "{} {}",
                    p.bar(used, 12),
                    p.severity(used, &format!("{used:>3.0}%"))
                ),
                (_, None) => p.dim("no usage yet"),
            };
            println!(
                "  {} {:>5}  {} {} {} {}",
                fit(&name(&e.key), 28),
                e.count.map(|n| n.to_string()).unwrap_or_default(),
                if e.kind == Kind::Spare {
                    p.info(&what)
                } else {
                    what
                },
                if soon || ends { p.warn(&when) } else { when },
                fit(&date, date_w),
                last
            );
        }
    };

    let limits = view::limit_resets(&c.reg, &c.logged_in, c.now);
    table(
        "Spare resets",
        true,
        ["RESET", "EXPIRES", "DATE", "USED"],
        &view::grants(&c.reg, c.now),
        "none",
    );
    println!();
    table(
        if limits.iter().any(|e| e.what == "monthly") {
            "Weekly and monthly resets"
        } else {
            "Weekly resets"
        },
        false,
        ["RESET", "IN", "DATE", "USED"],
        &limits,
        "nothing yet: run `cswap refresh`",
    );
    println!();
    table(
        "Renewals",
        false,
        ["PLAN", "IN", "DATE", ""],
        &view::renewals(&c.reg),
        "no renewal date is known",
    );
    println!();
    println!(
        "{}",
        p.dim(
            "cswap only shows resets. Spending one happens in the agent or on the vendor's site."
        )
    );
    Ok(())
}

// -------------------------------------------------------------------- suggest

pub fn suggest_cmd(agent: Option<&str>) -> Result<()> {
    let c = Ctx::load()?;
    let p = &c.p;
    let agents = match agent {
        Some(a) => vec![harness(a)?],
        None => c.agents(),
    };
    if agents.is_empty() {
        println!("{NO_ACCOUNTS}");
        return Ok(());
    }
    for (i, h) in agents.into_iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{}", p.bold(h.name()));
        let accounts: Vec<&Account> = c.reg.of(h).filter(|a| !a.is_native()).collect();
        if accounts.is_empty() {
            println!("  {}", p.dim("no account yet: cswap add <agent> <name>"));
            continue;
        }
        let ranked = suggest::rank(&accounts, &c.settings, c.now);
        let mut first = true;
        for (n, pick) in ranked.iter().enumerate() {
            let Some(acc) = c.reg.get(&pick.key) else {
                continue;
            };
            let best = pick.usable && first;
            if pick.usable {
                first = false;
            }
            // An arrow, not a star: the star marks an agent's default.
            let star = if best {
                p.accent("▸")
            } else {
                " ".to_string()
            };
            let name = fit(&acc.title(), 18);
            let why = pick.reasons.join(" · ");
            println!(
                "  {}. {star} {} {}",
                n + 1,
                if best { p.bold(&name) } else { name },
                if pick.usable { why } else { p.dim(&why) }
            );
            if best {
                println!(
                    "       {} {}",
                    p.dim("run:"),
                    p.info(&view::launch_hint(acc, c.is_default(acc)))
                );
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------- ps

pub fn ps(as_json: bool) -> Result<()> {
    let c = Ctx::load()?;
    let p = &c.p;
    let mut rows = Vec::new();
    for acc in &c.reg.accounts {
        for pid in c.live.get(&acc.key()).into_iter().flatten() {
            rows.push((acc, *pid, sessions::details(*pid)));
        }
    }
    if as_json {
        let all: Vec<_> = rows
            .iter()
            .map(|(acc, pid, d)| {
                json!({
                    "agent": acc.harness.id(),
                    "account": acc.key(),
                    "alias": acc.meta.label,
                    "pid": pid,
                    "cwd": d.cwd,
                    "started": d.started,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("No agent is running.");
        if !cfg!(target_os = "linux") {
            println!(
                "{}",
                p.dim("On this system only agents started through cswap are seen.")
            );
        }
        return Ok(());
    }
    println!(
        "{}",
        p.dim(&format!(
            "{} {} {} {} {}",
            fit("AGENT", 13),
            fit("ACCOUNT", 24),
            fit_right("PID", 8),
            fit("  SINCE", 11),
            "FOLDER"
        ))
    );
    for (acc, pid, d) in &rows {
        let account = format!("{} ({})", acc.title(), acc.key());
        let since = d
            .started
            .map(|t| model::ago(t, c.now))
            .unwrap_or_else(|| "–".to_string());
        let cwd = d
            .cwd
            .as_deref()
            .map(paths::tilde)
            .unwrap_or_else(|| "–".to_string());
        println!(
            "{} {} {}   {} {}",
            p.bold(&fit(acc.harness.name(), 13)),
            fit(&account, 24),
            fit_right(&pid.to_string(), 8),
            p.dim(&fit(&since, 8)),
            cwd
        );
    }
    println!();
    let agents: HashSet<Harness> = rows.iter().map(|(a, _, _)| a.harness).collect();
    println!(
        "{}",
        p.dim(&format!(
            "{} on {}",
            plural(rows.len(), "session"),
            plural(agents.len(), "agent")
        ))
    );
    Ok(())
}

// ------------------------------------------------------------------- accounts

fn confirm(question: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("this needs a yes: add -y to go ahead without asking");
    }
    eprint!("{question} [y/N] ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Ask for one line on the terminal.
fn ask(question: &str) -> Result<String> {
    if !std::io::stdin().is_terminal() {
        bail!("an account needs a name: cswap add <agent> <name>");
    }
    eprint!("{question} ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn after_sign_in(key: &str) -> Result<()> {
    let reg = Registry::load()?;
    let acc = reg.get(key).context("the account is gone")?;
    let p = Paint::stdout();
    let who = match acc.email() {
        "" => String::new(),
        e => format!(" as {e}"),
    };
    println!("{} {} is signed in{who}.", p.ok("✓"), acc.title());
    if let Some(other) = manage::duplicate_of(&reg, acc) {
        println!(
            "{} {} holds the same login. Keep one of the two: they share one limit, \
             and one login in two places stops working when it renews.",
            p.warn("▲"),
            other.title()
        );
    }
    let hs = HarnessSettings::load(acc.harness)?;
    println!(
        "  Start it with: {}",
        p.info(&view::launch_hint(acc, view::is_default(acc, &hs)))
    );
    Ok(())
}

pub fn add(agent: &str, name: Option<&str>) -> Result<()> {
    let h = harness(agent)?;
    manage::find_bin(h, &HarnessSettings::load(h)?)?;
    let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => ask(&format!("Name for the new {} account:", h.name()))?,
    };
    let mut acc = manage::create(h, &name)?;
    let key = acc.key();
    eprintln!("{}", manage::sign_in_banner(&acc));
    let undo = |acc: &Account| {
        let _ = manage::remove(acc, &Live::new());
    };
    match manage::sign_in(&mut acc) {
        Ok(true) => {
            // One account per login. The agent's own default is the one
            // place the same login may also sit.
            let reg = Registry::load()?;
            if let Some(other) = manage::duplicate_of(&reg, &acc) {
                let (other, who) = (other.title(), acc.email().to_string());
                undo(&acc);
                bail!(
                    "{other} already holds the login {who}, so '{name}' was not added. \
                     One account per login"
                );
            }
            after_sign_in(&key)
        }
        Ok(false) => {
            undo(&acc);
            bail!("the sign-in was not completed, so nothing was added");
        }
        Err(e) => {
            undo(&acc);
            Err(e)
        }
    }
}

pub fn login(account: &str) -> Result<()> {
    let mut reg = Registry::load()?;
    let key = reg.find_any(account)?.key();
    let live = sessions::scan(&reg);
    let acc = reg.get_mut(&key).context("the account is gone")?;
    if acc.is_native() {
        bail!(
            "{key} is {}'s root login. Sign in by running `{}` itself",
            acc.harness.name(),
            acc.harness.spec().bins[0]
        );
    }
    if live.get(&key).is_some_and(|p| !p.is_empty()) {
        bail!("{key} has a session running. Close it first");
    }
    eprintln!("{}", manage::sign_in_banner(acc));
    if !manage::sign_in(acc)? {
        bail!("the sign-in was not completed. {key} is unchanged");
    }
    after_sign_in(&key)
}

pub fn remove(account: &str, yes: bool) -> Result<()> {
    let reg = Registry::load()?;
    let acc = reg.find_any(account)?;
    let live = sessions::scan(&reg);
    if acc.is_native() {
        // Let `manage::remove` say why.
        return manage::remove(acc, &live);
    }
    let who = match acc.email() {
        "" => String::new(),
        e => format!(" ({e})"),
    };
    if !yes
        && !confirm(&format!(
            "Remove {} · {}{who}? Its login is deleted; your {} data is not touched.",
            acc.key(),
            acc.title(),
            paths::tilde(&acc.harness.native_dir())
        ))?
    {
        println!("Nothing was removed.");
        return Ok(());
    }
    manage::remove(acc, &live)?;
    println!("{} removed.", acc.key());
    Ok(())
}

pub fn rename(account: &str, name: &str) -> Result<()> {
    let mut reg = Registry::load()?;
    let key = reg.find_any(account)?.key();
    let acc = reg.get_mut(&key).context("the account is gone")?;
    manage::rename(acc, name)?;
    println!(
        "{key} is now '{}'. Use it as: {} --cswap={}",
        acc.title(),
        acc.harness.spec().bins[0],
        acc.title()
    );
    Ok(())
}

/// Show an account's aliases, replace them, or clear them.
pub fn alias(account: &str, aliases: &[String], clear: bool) -> Result<()> {
    let mut reg = Registry::load()?;
    let key = reg.find_any(account)?.key();
    let acc = reg.get_mut(&key).context("the account is gone")?;
    if clear || !aliases.is_empty() {
        manage::set_aliases(acc, aliases)?;
    }
    match acc.meta.aliases.as_slice() {
        [] => println!(
            "{} has no alias. Its name is all it answers to.",
            acc.title()
        ),
        list => println!("{} also answers to: {}", acc.title(), list.join(", ")),
    }
    Ok(())
}

/// `cswap use`: an agent's default, the account a launch that names none starts on.
pub fn use_cmd(agent: &str, account: Option<&str>) -> Result<()> {
    let h = harness(agent)?;
    let bin = h.spec().bins[0];
    let describe = |value: &str| -> Result<String> {
        Ok(match value {
            "root" => format!("{}'s root login", h.name()),
            "auto" => "the best account at every launch (auto)".to_string(),
            id => {
                let reg = Registry::load()?;
                let found = reg.of(h).find(|a| a.meta.id.to_string() == id);
                let text = match found {
                    Some(a) => format!("{} ({})", a.title(), a.key()),
                    None => format!("account {id}, which no longer exists"),
                };
                text
            }
        })
    };
    match account {
        None => {
            let hs = HarnessSettings::load(h)?;
            println!("`{bin}` starts on {}.", describe(&hs.start)?);
            println!(
                "Change it with: cswap use {} <account | root | auto>",
                h.id()
            );
        }
        Some(sel) => {
            let value = manage::set_start(h, sel)?;
            println!("`{bin}` now starts on {}.", describe(&value)?);
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------- sync

pub fn sync_cmd(account: Option<&str>, repair: bool, check_only: bool) -> Result<()> {
    let mut reg = Registry::load()?;
    let live = sessions::scan(&reg);
    let keys: Vec<String> = match account {
        Some(sel) => {
            let acc = reg.find_any(sel)?;
            if acc.is_native() {
                bail!(
                    "{} is {}'s own login: it uses {} directly and has nothing to sync",
                    acc.key(),
                    acc.harness.name(),
                    paths::tilde(&acc.harness.native_dir())
                );
            }
            vec![acc.key()]
        }
        None => reg
            .accounts
            .iter()
            .filter(|a| !a.is_native())
            .map(|a| a.key())
            .collect(),
    };
    if keys.is_empty() {
        println!("No cswap accounts yet, so nothing to sync. Add one with: cswap add <agent>");
        return Ok(());
    }
    let p = Paint::stdout();
    let mut left_shadowed = 0;
    for key in keys {
        let acc = reg.get_mut(&key).context("the account is gone")?;
        let h = acc.harness;
        let hs = HarnessSettings::load(h)?;
        let mut parked = None;
        let report = if check_only {
            sync::check_account(acc, &hs)?
        } else {
            let mut report = sync::sync_account(acc, &hs)?;
            if repair && report.count(State::Shadowed) > 0 {
                if live.get(&key).is_some_and(|p| !p.is_empty()) {
                    println!(
                        "{} {key} has a session running; close it before a repair.",
                        p.warn("▲")
                    );
                } else {
                    parked = sync::repair(h, &key, &acc.config_dir(), &hs.sync.exclude)?;
                    report = sync::check_account(acc, &hs)?;
                }
            }
            HarnessSettings::update(h, |s| s.sync.last_synced = Some(Utc::now()))?;
            report
        };
        let shadowed = report.names(State::Shadowed);
        let mark = if report.clean() {
            p.ok("✓")
        } else if shadowed.is_empty() {
            p.info("○")
        } else {
            p.warn("▲")
        };
        let mut parts = vec![
            format!("{} linked", report.count(State::Linked)),
            format!("{} private", report.count(State::Private)),
        ];
        if report.pending() > 0 {
            parts.push(format!("{} to link", report.pending()));
        }
        println!(
            "{mark} {} {}  {}  {}",
            fit(&key, 10),
            fit(&acc.title(), 18),
            parts.join(" · "),
            p.dim(&format!("→ {}", paths::tilde(&h.native_dir())))
        );
        if let Some(dir) = parked {
            println!("    moved the shadowing files to {}", paths::tilde(&dir));
        }
        if !shadowed.is_empty() {
            left_shadowed += shadowed.len();
            println!(
                "    {} {}",
                p.warn("keeps its own copy of:"),
                shadowed.join(", ")
            );
        }
    }
    if left_shadowed > 0 {
        println!();
        println!(
            "A real file sits where a link belongs, so that part is not shared. `cswap sync --repair`\nmoves such files to ~/.cswap/parked/ and relinks. Nothing is deleted or merged."
        );
    }
    Ok(())
}

// --------------------------------------------------------------------- doctor

pub fn doctor() -> Result<bool> {
    let c = Ctx::load()?;
    let p = &c.p;
    let checks = health::run(&c.reg, &c.live);
    let mut area = String::new();
    for check in &checks {
        if check.area != area {
            if !area.is_empty() {
                println!();
            }
            println!("{}", p.bold(&check.area));
            area = check.area.clone();
        }
        let mark = match check.level {
            Level::Ok => p.ok("✓"),
            Level::Info => p.dim("·"),
            Level::Warn => p.warn("▲"),
            Level::Bad => p.bad("✗"),
        };
        println!("  {mark} {}", check.title);
        if !check.detail.is_empty() && check.level >= Level::Warn {
            println!("      {}", p.dim(&check.detail));
        }
    }
    println!();
    let worst = health::worst(&checks);
    let n = health::attention(&checks);
    match worst {
        Level::Bad => println!(
            "{}",
            p.bad(&format!("{} to fix or look at.", plural(n, "thing")))
        ),
        Level::Warn => println!("{}", p.warn(&format!("{} to look at.", plural(n, "thing")))),
        _ => println!("{}", p.ok("All clear.")),
    }
    Ok(worst != Level::Bad)
}

// ---------------------------------------------------------------------- start

/// `cswap start`: ask for the agent, then the account, then become it.
pub fn start(default_only: bool, agent: Option<&str>, args: Vec<String>) -> Result<()> {
    let reg = Registry::load()?;
    let h = match agent {
        Some(a) => harness(a)?,
        None => match picker::pick_harness(&reg)? {
            Some(h) => h,
            None => return Ok(()),
        },
    };
    let mut argv = Vec::new();
    if !default_only {
        // With a single account there is nothing to ask.
        let n = reg.of(h).count();
        if n > 1 {
            let settings = Settings::load()?;
            match picker::pick(&reg, h, &settings)? {
                Some(key) => {
                    let acc = reg.get(&key).context("the account is gone")?;
                    argv.push(format!("--cswap={}", acc.key()));
                }
                None => return Ok(()),
            }
        }
    }
    argv.extend(args);
    wrap::run(h, argv)
}

// ----------------------------------------------------------------- shell-init

pub fn shell_init(shell: &str) -> Result<()> {
    let settings = Settings::load()?;
    // A function for every agent this shell wraps that is installed: the
    // ones on its list, and the ones with a PATH shim, which cover every
    // shell.
    let agents: Vec<Harness> = shell::wrapped(&settings, shell, |h| {
        HarnessSettings::load(h).map(|s| s.wrapped).unwrap_or(true)
    })
    .into_iter()
    .filter(|h| manage::installed(*h))
    .collect();
    print!("{}", shell::init_text(shell, &agents)?);
    // With shims on, their folder goes first again after the rc file's own
    // changes to PATH.
    print!("{}", crate::shim::init_text(shell, &settings));
    Ok(())
}

/// One line per agent: whether a bare launch reaches cswap from here.
fn shim_lines(p: &Paint, agents: &[Harness]) {
    use crate::shim::State;
    for h in agents {
        let bin = h.spec().bins[0];
        match crate::shim::state(*h) {
            State::Active => println!("  {} {bin} runs through cswap from here", p.ok("✓")),
            State::Broken => println!(
                "  {} {bin}: its shim is missing or leads to another cswap. Run: cswap shims on",
                p.warn("▲")
            ),
            State::Shadowed(first) => println!(
                "  {} {bin}: {} comes first on PATH here, so a bare `{bin}` skips cswap",
                p.warn("▲"),
                paths::tilde(&first)
            ),
            State::OffPath => println!(
                "  {} {bin}: {} is not on PATH here yet",
                p.warn("▲"),
                paths::tilde(&crate::shim::dir())
            ),
        }
    }
}

/// One line per agent: what herdr itself starts for its bare name. False when
/// herdr answers and any of them is not cswap. Silent when herdr is not here.
fn herdr_lines(p: &Paint, agents: &[Harness]) -> bool {
    let mut ok = true;
    for (h, first, ours) in crate::shim::seen_by_herdr(agents).unwrap_or_default() {
        let bin = h.spec().bins[0];
        if ours {
            println!("  {} herdr starts a bare {bin} through cswap", p.ok("✓"));
            continue;
        }
        ok = false;
        match first {
            Some(first) => println!(
                "  {} herdr still starts {} for a bare {bin}: it keeps the PATH it started with",
                p.warn("▲"),
                paths::tilde(&first)
            ),
            None => println!("  {} herdr finds no {bin} at all", p.warn("▲")),
        }
    }
    ok
}

/// What has to happen before a new PATH is seen, said once.
fn shim_when(p: &Paint) {
    println!(
        "{}",
        p.dim("A new terminal sees it at once when its shell loads cswap, and every program after your next login.")
    );
    println!(
        "{}",
        p.dim("herdr keeps the PATH it started with. To give it the new one now, run in a terminal outside herdr:")
    );
    println!("{}", p.dim("  hc server restart --fresh"));
}

/// `cswap shims`: where the PATH shims stand.
pub fn shims_status() -> Result<bool> {
    let p = Paint::stdout();
    let settings = Settings::load()?;
    if !crate::shim::supported() {
        println!("PATH shims are for Linux and macOS. On Windows, use the shell integration.");
        return Ok(true);
    }
    let agents = crate::shim::chosen(&settings);
    if agents.is_empty() {
        println!("PATH shims are off.");
        println!(
            "{}",
            p.dim("With them on, an agent started by any program (herdr, a script) goes through cswap, not only one typed in a shell.")
        );
        println!("{}", p.dim("Turn them on with: cswap shims on"));
        return Ok(true);
    }
    println!(
        "PATH shims are on: {} in {}",
        agents
            .iter()
            .map(|h| h.spec().bins[0])
            .collect::<Vec<_>>()
            .join(", "),
        paths::tilde(&crate::shim::dir())
    );
    shim_lines(&p, &agents);
    let mut ok = agents
        .iter()
        .all(|h| crate::shim::state(*h) == crate::shim::State::Active);
    ok &= herdr_lines(&p, &agents);
    if !ok {
        shim_when(&p);
    }
    Ok(ok)
}

/// `cswap shims on [agents…]`: every installed agent when none is named.
pub fn shims_on(names: &[String]) -> Result<()> {
    let p = Paint::stdout();
    let agents: Vec<Harness> = if names.is_empty() {
        Harness::ALL
            .into_iter()
            .filter(|h| manage::installed(*h))
            .collect()
    } else {
        let mut out = Vec::new();
        for name in names {
            let h = harness(name)?;
            if !out.contains(&h) {
                out.push(h);
            }
        }
        out
    };
    if agents.is_empty() {
        bail!("no agent is installed, so there is nothing to make a shim for");
    }
    let saved = crate::shim::set(&agents)?;
    println!(
        "PATH shims are on for {}.",
        agents
            .iter()
            .map(|h| h.spec().bins[0])
            .collect::<Vec<_>>()
            .join(", ")
    );
    for file in &saved.added_to {
        println!("Added the PATH line to {}.", paths::tilde(file));
    }
    shim_lines(&p, &agents);
    herdr_lines(&p, &agents);
    shim_when(&p);
    println!(
        "{}",
        p.dim("These agents now run through cswap in every shell, so their per-shell choice in Settings > Shell is locked. The lists are kept.")
    );
    println!("{}", p.dim("Undo all of it with: cswap shims off"));
    Ok(())
}

/// `cswap shims off`: the links, the folder and the PATH lines go.
pub fn shims_off() -> Result<()> {
    let saved = crate::shim::set(&[])?;
    println!("PATH shims are off. The links are removed.");
    for file in &saved.removed_from {
        println!("Took the PATH line out of {}.", paths::tilde(file));
    }
    println!(
        "{}",
        Paint::stdout().dim("Programs that are already running keep the PATH they have; a shim that is gone is skipped, so the real agent starts.")
    );
    Ok(())
}

/// `cswap uninstall`: undo what cswap wrote outside its own folder, and say
/// what is left. The logins and the binary are the user's to delete.
pub fn uninstall(yes: bool) -> Result<()> {
    let p = Paint::stdout();
    if !yes
        && !confirm("Remove cswap's PATH shims and the cswap lines in your shell and login files?")?
    {
        println!("Nothing was changed.");
        return Ok(());
    }
    if crate::shim::supported() {
        let saved = crate::shim::set(&[])?;
        println!("PATH shims: removed.");
        for file in &saved.removed_from {
            println!("  took the PATH line out of {}", paths::tilde(file));
        }
    }
    for name in shell::KNOWN {
        if shell::remove_line(name)? {
            if let Some(rc) = shell::rc_file(name) {
                println!(
                    "Shell integration: took the cswap line out of {}",
                    paths::tilde(&rc)
                );
            }
        }
    }
    Settings::update(|s| s.shells.clear())?;

    println!();
    println!(
        "{}",
        p.bold("Left for you, because cswap cannot undo them:")
    );
    let exe = std::env::current_exe().ok();
    if let Some(exe) = &exe {
        println!("  the binary          rm {}", paths::tilde(exe));
    }
    println!(
        "  accounts and logins rm -rf {}",
        paths::tilde(&paths::root())
    );
    println!(
        "{}",
        p.dim("      That folder holds every account's login. Sign in to what you still need first, with the agent itself.")
    );
    if matches!(
        crate::integration::state(),
        crate::integration::State::Installed { .. }
    ) {
        let home = paths::tilde(&crate::integration::herdr_home());
        println!("  the herdr layout    switch herdr to another layout, then:");
        println!("                      rm -r {home}/layouts/cswap.* {home}/mods/cswap-*");
    }
    println!(
        "{}",
        p.dim("Open terminals keep the wrappers until they are closed. The agents' own folders were never changed.")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_are_named_by_id_binary_or_product() {
        assert_eq!(harness("claude").unwrap(), Harness::Claude);
        assert_eq!(harness("agy").unwrap(), Harness::Agy);
        assert_eq!(harness("antigravity").unwrap(), Harness::Agy);
        assert!(harness("vim").is_err());
    }

    #[test]
    fn plural_adds_an_s_only_when_needed() {
        assert_eq!(plural(1, "session"), "1 session");
        assert_eq!(plural(3, "session"), "3 sessions");
        assert_eq!(plural(0, "x"), "0 xs");
    }
}
