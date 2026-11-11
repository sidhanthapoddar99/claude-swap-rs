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
use crate::{manage, model, paths, picker, refresh, suggest, wrap};

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

    fn signed_in(&self, acc: &Account) -> bool {
        self.logged_in.contains(&acc.key())
    }

    fn is_default(&self, acc: &Account) -> bool {
        view::is_default(acc, &self.hs[&acc.harness])
    }

    fn state(&self, acc: &Account) -> view::State {
        view::state(
            acc,
            self.signed_in(acc),
            self.live.get(&acc.key()).map_or(0, Vec::len),
            false,
            self.now,
        )
    }

    fn state_text(&self, acc: &Account) -> String {
        let s = self.state(acc);
        let text = s.label();
        match s {
            view::State::Live(_) => self.p.ok(&text),
            view::State::Blocked => self.p.bad(&text),
            view::State::Stale => self.p.warn(&text),
            _ => self.p.dim(&text),
        }
    }

    /// The agents that have at least one account, in their fixed order.
    fn agents(&self) -> Vec<Harness> {
        Harness::ALL
            .into_iter()
            .filter(|h| self.reg.of(*h).next().is_some())
            .collect()
    }

    fn agent_header(&self, h: Harness) -> String {
        let hs = &self.hs[&h];
        let default = match hs.default.as_str() {
            "native" => "its own login".to_string(),
            "auto" => "auto".to_string(),
            id => self
                .reg
                .of(h)
                .find(|a| a.meta.id.to_string() == id)
                .map(|a| a.title())
                .unwrap_or_else(|| format!("{id} (missing)")),
        };
        let mut notes = vec![format!("`{}` starts on {default}", h.spec().bins[0])];
        if !hs.wrapped {
            notes.push("not wrapped".to_string());
        }
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

fn account_json(c: &Ctx, acc: &Account) -> serde_json::Value {
    let snap = acc.snapshot();
    json!({
        "key": acc.key(),
        "agent": acc.harness.id(),
        "id": acc.meta.id,
        "alias": acc.meta.label,
        "email": acc.meta.identity.email,
        "plan": view::plan_text(acc, &c.local),
        "default": c.is_default(acc),
        "signed_in": c.signed_in(acc),
        "running": c.live.get(&acc.key()).cloned().unwrap_or_default(),
        "state": c.state(acc).label(),
        "fetched_at": snap.map(|s| s.fetched_at),
        "windows": snap.map(|s| s.windows.iter().map(|w| json!({
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
                Some(at) if at > c.now => format!("resets in {}", model::span(at - c.now)),
                Some(_) => "has reset".to_string(),
                None => String::new(),
            };
            format!(
                "{} {}  {}",
                p.bar(used, 12),
                p.severity(used, &format!("{used:>3.0}%")),
                p.dim(&fit(&reset, 18))
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
            p.dim(&fit(&text, 37))
        }
    }
}

pub fn list(as_json: bool) -> Result<()> {
    let c = Ctx::load()?;
    if as_json {
        let all: Vec<_> = c.reg.accounts.iter().map(|a| account_json(&c, a)).collect();
        println!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(());
    }
    if c.reg.accounts.is_empty() {
        println!("No agent found. cswap looks for claude, codex, agy and grok.");
        return Ok(());
    }
    let p = &c.p;
    for (i, h) in c.agents().into_iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{}", c.agent_header(h));
        for acc in c.reg.of(h) {
            let star = if c.is_default(acc) {
                p.accent("★")
            } else {
                " ".to_string()
            };
            let title = acc.title();
            let email = if acc.email() == title {
                ""
            } else {
                acc.email()
            };
            println!(
                "  {star} {} {} {} {} {}  {}",
                p.dim(&fit_right(&id_text(acc), 2)),
                p.bold(&fit(&title, 18)),
                p.dim(&fit(email, 28)),
                fit(&view::plan_text(acc, &c.local), 10),
                usage_cell(&c, acc),
                c.state_text(acc),
            );
        }
    }
    println!();
    println!(
        "{}",
        p.dim("★ default · `cswap usage` for every limit · `cswap` alone opens the dashboard")
    );
    Ok(())
}

// ---------------------------------------------------------------------- usage

fn fetch(keys: &[String]) -> Vec<(String, Option<String>)> {
    std::thread::scope(|s| {
        let jobs: Vec<_> = keys
            .iter()
            .map(|key| {
                s.spawn(move || {
                    let run = || -> Result<(), String> {
                        let mut reg = Registry::load().map_err(|e| format!("{e:#}"))?;
                        let acc = reg.get_mut(key).ok_or("the account is gone")?;
                        refresh::refresh_account(acc).map_err(|e| e.text)
                    };
                    (key.clone(), run().err())
                })
            })
            .collect();
        jobs.into_iter()
            .map(|j| {
                j.join()
                    .unwrap_or_else(|_| (String::new(), Some("the fetch crashed".into())))
            })
            .collect()
    })
}

/// The keys to act on: one named account, or every signed-in one.
fn targets(c: &Ctx, account: Option<&str>) -> Result<Vec<String>> {
    match account {
        Some(sel) => Ok(vec![c.reg.find_any(sel)?.key()]),
        None => Ok(c
            .reg
            .accounts
            .iter()
            .filter(|a| c.signed_in(a))
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
        None => c.reg.accounts.iter().collect(),
    };
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
        let main = view::main_window(snap).map(|w| w.label.clone());
        // The long windows first; the short one last, as in the dashboard.
        let mut windows: Vec<_> = snap.windows.iter().collect();
        windows.sort_by_key(|w| w.kind == crate::model::WindowKind::Session);
        for w in windows {
            let used = w.used_now(c.now);
            let reset = match w.resets_at {
                Some(at) if at > c.now => format!(
                    "{} · in {}",
                    model::landing(at, c.now, c.settings.clock_24h),
                    model::span(at - c.now)
                ),
                Some(_) => "has reset since the last fetch".to_string(),
                None => String::new(),
            };
            println!(
                "  {} {} {}  {}",
                fit(&w.label, 12),
                p.bar(used, 24),
                p.severity(used, &format!("{used:>3.0}%")),
                reset
            );
            if Some(&w.label) == main.as_ref() {
                if let Some((text, hot)) = view::pace_text(w, c.now, c.settings.clock_24h) {
                    let pad = " ".repeat(15);
                    println!("{pad}{}", if hot { p.warn(&text) } else { p.dim(&text) });
                }
            }
        }
        if let Some(text) = snap.credits.as_ref().and_then(view::credits_text) {
            println!("  {} {text}", fit("extra usage", 12));
        }
        let grants = acc.grants(c.now);
        if !grants.is_empty() {
            let n: u32 = grants.iter().map(|g| g.count).sum();
            let soonest = grants.iter().filter_map(|g| g.expires_at).min();
            let mut text = plural(n as usize, "spare reset");
            if let Some(at) = soonest {
                text.push_str(&format!(
                    " · the first expires in {}",
                    model::span(at - c.now)
                ));
            }
            println!("  {} {}", fit("resets", 12), p.info(&text));
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
    let name = |key: &str| -> String {
        c.reg
            .get(key)
            .map(|a| format!("{} · {}", a.title(), a.harness.name()))
            .unwrap_or_else(|| key.to_string())
    };

    let grants = view::grants(&c.reg, c.now);
    println!("{}", p.bold("Spare resets"));
    if grants.is_empty() {
        println!("  {}", p.dim("none"));
    }
    for g in &grants {
        let when = match g.at {
            Some(at) => format!(
                "expires {} · in {}",
                model::landing(at, c.now, c.settings.clock_24h),
                model::span(at - c.now)
            ),
            None => "does not expire".to_string(),
        };
        let urgent =
            g.at.is_some_and(|at| at - c.now <= chrono::Duration::days(3));
        println!(
            "  {} {} {}  {}",
            fit(&name(&g.key), 30),
            p.info(&fit(&g.what, 18)),
            if urgent {
                p.warn(&fit(&when, 40))
            } else {
                fit(&when, 40)
            },
            p.dim(&g.note)
        );
    }
    println!();
    println!("{}", p.bold("Coming up"));
    let coming = view::upcoming(&c.reg, &c.logged_in, &c.local, c.now);
    if coming.is_empty() {
        println!("  {}", p.dim("nothing yet: run `cswap refresh`"));
    }
    for e in &coming {
        let what = match e.kind {
            Kind::PlanEnds | Kind::LoginExpires => p.warn(&fit(&e.what, 20)),
            _ => fit(&e.what, 20),
        };
        println!(
            "  {} {} {} {what} {}",
            fit(&e.when(c.now, c.settings.clock_24h), 19),
            p.dim(&fit(&format!("in {}", e.countdown(c.now)), 12)),
            fit(&name(&e.key), 30),
            p.dim(&e.note)
        );
    }
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
    for (i, h) in agents.into_iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{}", p.bold(h.name()));
        let accounts: Vec<&Account> = c.reg.of(h).collect();
        if accounts.is_empty() {
            println!("  {}", p.dim("no account"));
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
            let star = if best {
                p.accent("★")
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

fn after_sign_in(key: &str) -> Result<()> {
    let reg = Registry::load()?;
    let acc = reg.get(key).context("the account is gone")?;
    let p = Paint::stdout();
    let who = match acc.email() {
        "" => "a login that names no one".to_string(),
        e => e.to_string(),
    };
    println!("{} {key} is signed in as {who}.", p.ok("✓"));
    if let Some(other) = manage::duplicate_of(&reg, acc) {
        println!(
            "{} That is the same login as {}. Two accounts on one login share one limit.",
            p.warn("▲"),
            other.key()
        );
    }
    let hs = HarnessSettings::load(acc.harness)?;
    println!(
        "  Start it with: {}",
        p.info(&view::launch_hint(acc, view::is_default(acc, &hs)))
    );
    Ok(())
}

pub fn add(agent: &str, alias: Option<&str>) -> Result<()> {
    let h = harness(agent)?;
    manage::find_bin(h, &HarnessSettings::load(h)?)?;
    let mut acc = manage::create(h, alias)?;
    let key = acc.key();
    eprintln!("{}", manage::sign_in_banner(&acc));
    let undo = |acc: &Account| {
        let _ = manage::remove(acc, &Live::new());
    };
    match manage::sign_in(&mut acc) {
        Ok(true) => after_sign_in(&key),
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
            "{key} is {}'s own login. Sign in by running `{}` itself",
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

pub fn alias(account: &str, name: Option<&str>) -> Result<()> {
    let mut reg = Registry::load()?;
    let key = reg.find_any(account)?.key();
    let acc = reg.get_mut(&key).context("the account is gone")?;
    let name = name.unwrap_or("").trim();
    manage::set_label(acc, name)?;
    if name.is_empty() {
        println!("{key}: alias cleared.");
    } else {
        println!(
            "{key} is now '{name}'. Use it as: {} --cswap={name}",
            acc.harness.spec().bins[0]
        );
    }
    Ok(())
}

pub fn default(agent: &str, account: Option<&str>) -> Result<()> {
    let h = harness(agent)?;
    let bin = h.spec().bins[0];
    let describe = |value: &str| -> Result<String> {
        Ok(match value {
            "native" => format!("{}'s own login", h.name()),
            "auto" => "the best pick at every launch (auto)".to_string(),
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
            println!("`{bin}` starts on {}.", describe(&hs.default)?);
            println!(
                "Change it with: cswap default {} <account | native | auto>",
                h.id()
            );
        }
        Some(sel) => {
            let value = manage::set_default(h, sel)?;
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
    let bins: Vec<(&str, &str)> = Harness::ALL
        .into_iter()
        .filter(|h| manage::installed(*h))
        .flat_map(|h| h.spec().bins.iter().map(move |b| (*b, h.id())))
        .collect();
    let mut out = String::new();
    match shell {
        "bash" | "zsh" | "sh" => {
            out.push_str("# cswap shell integration. Each agent runs through cswap, which picks the account.\n");
            for (bin, id) in &bins {
                // An alias of the same name would win over the function.
                out.push_str(&format!(
                    "unalias {bin} 2>/dev/null\n{bin}() {{ command cswap {id} \"$@\"; }}\n"
                ));
            }
        }
        "fish" => {
            out.push_str("# cswap shell integration. Each agent runs through cswap, which picks the account.\n");
            for (bin, id) in &bins {
                out.push_str(&format!(
                    "function {bin} --description 'cswap: {bin}'\n    command cswap {id} $argv\nend\n"
                ));
            }
        }
        "powershell" | "pwsh" => {
            out.push_str("# cswap shell integration. Each agent runs through cswap, which picks the account.\n");
            for (bin, id) in &bins {
                out.push_str(&format!("function {bin} {{ cswap.exe {id} @args }}\n"));
            }
        }
        other => bail!("'{other}' is not a shell cswap knows. Use bash, zsh, fish or powershell"),
    }
    print!("{out}");
    Ok(())
}

/// The line to put in an rc file for `shell`.
pub fn shell_hint(shell: &str) -> String {
    match shell {
        "fish" => "cswap shell-init fish | source".to_string(),
        "powershell" | "pwsh" => {
            "Invoke-Expression (& cswap shell-init powershell | Out-String)".to_string()
        }
        "zsh" => "eval \"$(cswap shell-init zsh)\"".to_string(),
        _ => "eval \"$(cswap shell-init bash)\"".to_string(),
    }
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
