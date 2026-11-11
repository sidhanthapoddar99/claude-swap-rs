//! What the dashboard and the text commands both show, worked out once: the
//! state of an account in a word, its plan, and the timeline of resets.

use chrono::{DateTime, Duration, TimeZone, Utc};
use std::collections::{HashMap, HashSet};

use crate::harness::Local;
use crate::model::{self, Snapshot, Source, Window, WindowKind};
use crate::store::{self, Account, HarnessSettings, Registry};

/// Which accounts are signed in, and what each login says about itself. Read
/// from disk in one pass, so drawing does not touch files.
///
/// An agent's own login can change without cswap seeing it, so for account 0
/// what is on disk now replaces what the last fetch stored (in memory only).
pub fn facts(reg: &mut Registry) -> (HashSet<String>, HashMap<String, Local>) {
    let mut logged_in = HashSet::new();
    let mut local = HashMap::new();
    for acc in &mut reg.accounts {
        let dir = acc.config_dir();
        if acc.harness.logged_in(&dir) {
            logged_in.insert(acc.key());
        }
        let l = acc.harness.local(&dir, acc.is_native());
        if acc.is_native() {
            crate::refresh::merge(&mut acc.meta, &l);
        }
        local.insert(acc.key(), l);
    }
    (logged_in, local)
}

/// Is `acc` what a bare launch of its agent uses?
pub fn is_default(acc: &Account, hs: &HarnessSettings) -> bool {
    match hs.default.as_str() {
        "native" => acc.is_native(),
        "auto" => false,
        id => id == acc.meta.id.to_string(),
    }
}

/// How to launch on this account from a shell.
pub fn launch_hint(acc: &Account, is_default: bool) -> String {
    let bin = acc.harness.spec().bins[0];
    if is_default {
        return bin.to_string();
    }
    let sel = if !acc.meta.label.is_empty() {
        acc.meta.label.clone()
    } else if acc.is_native() {
        "native".to_string()
    } else {
        acc.meta.id.to_string()
    };
    format!("{bin} --cswap={sel}")
}

/// The plan to show: what was typed by hand, else what the login says.
pub fn plan_text(acc: &Account, local: &HashMap<String, Local>) -> String {
    acc.meta
        .plan
        .clone()
        .or_else(|| local.get(&acc.key()).and_then(|l| l.plan.clone()))
        .unwrap_or_default()
}

/// The window an account is judged by in a one-line view: the weekly one, or
/// for an agent with no weekly limit the longest one it has.
pub fn main_window(snap: &Snapshot) -> Option<&Window> {
    snap.weekly()
        .or_else(|| snap.windows.iter().find(|w| w.kind != WindowKind::Session))
        .or_else(|| snap.windows.first())
}

/// What state an account is in, in one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    SignedOut,
    Fetching,
    Live(usize),
    Blocked,
    Stale,
    Ready,
}

impl State {
    pub fn label(self) -> String {
        match self {
            State::SignedOut => "signed out".to_string(),
            State::Fetching => "waiting".to_string(),
            State::Live(1) => "running".to_string(),
            State::Live(n) => format!("running ×{n}"),
            State::Blocked => "limit hit".to_string(),
            State::Stale => "stale".to_string(),
            State::Ready => "ready".to_string(),
        }
    }
}

/// `sessions` is how many agents run on the account now; `fetching` is true
/// while a refresh for it is in flight.
pub fn state(
    acc: &Account,
    signed_in: bool,
    sessions: usize,
    fetching: bool,
    now: DateTime<Utc>,
) -> State {
    if !signed_in {
        return State::SignedOut;
    }
    if sessions > 0 {
        return State::Live(sessions);
    }
    if fetching && acc.snapshot().is_none() {
        return State::Fetching;
    }
    match acc.snapshot() {
        Some(s) if s.blocking(now).is_some() => State::Blocked,
        Some(_) if acc.meta.refresh.error.is_some() => State::Stale,
        Some(_) => State::Ready,
        None if acc.meta.refresh.error.is_some() => State::Stale,
        None => State::Fetching,
    }
}

/// One sentence on whether usage is ahead of or behind the clock. The flag is
/// true when the limit will be hit before the reset.
pub fn pace_text(w: &Window, now: DateTime<Utc>, clock_24h: bool) -> Option<(String, bool)> {
    let elapsed = w.elapsed_pct(now)?;
    let used = w.used_now(now);
    let (reset, secs) = (w.resets_at?, w.window_secs?);
    if elapsed < 5.0 {
        return Some((format!("the {} has only just started", w.label), false));
    }
    if used >= 99.5 {
        return None;
    }
    let projected = w.projected_pct(now)?;
    if projected >= 100.0 && used > 0.0 {
        // Time from the window's start at which usage reaches 100%.
        let start = reset - Duration::seconds(secs);
        let elapsed_secs = (now - start).num_seconds() as f64;
        let runs_out = start + Duration::seconds((elapsed_secs * 100.0 / used) as i64);
        return Some((
            format!(
                "at this pace it runs out {}, {} before the reset",
                model::landing(runs_out, now, clock_24h),
                model::span(reset - runs_out)
            ),
            true,
        ));
    }
    Some((
        format!(
            "{used:.0}% used with {elapsed:.0}% of the {} gone: on pace for {projected:.0}% by the reset",
            w.label
        ),
        false,
    ))
}

fn money(c: &crate::model::Credits, v: f64) -> String {
    match c.unit.to_uppercase().as_str() {
        "USD" => format!("${v:.2}"),
        "" => format!("{v:.0}"),
        _ => format!("{v:.0} {}", c.unit),
    }
}

/// Pay-as-you-go usage in words, or `None` when it is off and empty.
pub fn credits_text(c: &crate::model::Credits) -> Option<String> {
    if !c.enabled && c.balance.is_none() && c.used.is_none() {
        return None;
    }
    Some(match (c.used, c.limit, c.balance) {
        (Some(u), Some(l), _) => format!("{} of {} spent", money(c, u), money(c, l)),
        (Some(u), None, _) => format!("{} spent, no cap", money(c, u)),
        (None, _, Some(b)) => format!("{} left", money(c, b)),
        _ => "on".to_string(),
    })
}

// ------------------------------------------------------------------ timeline

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A spare reset that can still be spent.
    Grant,
    /// A usage window going back to zero on its own.
    Window,
    Renewal,
    PlanEnds,
    LoginExpires,
}

/// One line of the resets view.
#[derive(Debug, Clone)]
pub struct Event {
    pub kind: Kind,
    pub key: String,
    /// When it happens; for a grant, when it expires.
    pub at: Option<DateTime<Utc>>,
    /// True when only the day is known.
    pub date_only: bool,
    pub what: String,
    pub note: String,
}

impl Event {
    /// `Sat 14 Nov · 09:30`, or just the day when the time is not known.
    pub fn when(&self, now: DateTime<Utc>, clock_24h: bool) -> String {
        match self.at {
            None => "never".to_string(),
            Some(at) if self.date_only => at
                .with_timezone(&chrono::Local)
                .format("%a %-d %b")
                .to_string(),
            Some(at) => model::landing(at, now, clock_24h),
        }
    }

    /// `2d 3h`, or whole days when only the day is known.
    pub fn countdown(&self, now: DateTime<Utc>) -> String {
        match self.at {
            None => "–".to_string(),
            Some(at) if self.date_only => {
                let days =
                    (at.with_timezone(&chrono::Local).date_naive() - store::today()).num_days();
                match days {
                    0 => "today".to_string(),
                    1 => "1 day".to_string(),
                    d => format!("{d} days"),
                }
            }
            Some(at) => model::span(at - now),
        }
    }
}

fn noon(date: chrono::NaiveDate) -> DateTime<Utc> {
    let naive = date.and_hms_opt(12, 0, 0).expect("valid time");
    chrono::Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|| naive.and_utc())
}

/// Every spare reset, soonest to expire first; the ones that never expire
/// come last.
pub fn grants(reg: &Registry, now: DateTime<Utc>) -> Vec<Event> {
    let mut out = Vec::new();
    for acc in &reg.accounts {
        for g in acc.grants(now) {
            let mut note = match g.source {
                Source::Api => format!("reported by {}", acc.harness.spec().vendor),
                Source::Manual => "noted by hand".to_string(),
            };
            match g.usable_now {
                Some(true) => note.push_str(" · can be spent now"),
                Some(false) => note.push_str(" · not spendable yet"),
                None => {}
            }
            let times = if g.count == 1 {
                String::new()
            } else {
                format!("{} × ", g.count)
            };
            out.push(Event {
                kind: Kind::Grant,
                key: acc.key(),
                at: g.expires_at,
                date_only: false,
                what: format!("{times}{} reset", g.scope),
                note,
            });
        }
    }
    out.sort_by_key(|r| (r.at.is_none(), r.at));
    out
}

/// How many spare resets all accounts hold together.
pub fn grant_count(reg: &Registry, now: DateTime<Utc>) -> u32 {
    reg.accounts
        .iter()
        .flat_map(|a| a.grants(now))
        .map(|g| g.count)
        .sum()
}

/// What resets, renews or runs out on its own, soonest first.
pub fn upcoming(
    reg: &Registry,
    logged_in: &HashSet<String>,
    local: &HashMap<String, Local>,
    now: DateTime<Utc>,
) -> Vec<Event> {
    let today = store::today();
    let mut out = Vec::new();
    for acc in &reg.accounts {
        let key = acc.key();
        if !logged_in.contains(&key) {
            continue;
        }
        if let Some(snap) = acc.snapshot() {
            for w in &snap.windows {
                // The short window comes round too often to be worth a line.
                if w.kind == WindowKind::Session {
                    continue;
                }
                let Some(at) = w.resets_at.filter(|t| *t > now) else {
                    continue;
                };
                out.push(Event {
                    kind: Kind::Window,
                    key: key.clone(),
                    at: Some(at),
                    date_only: false,
                    what: format!("{} limit resets", w.label),
                    note: format!("{:.0}% used now", w.used_now(now)),
                });
            }
        }
        let billing = &acc.meta.billing;
        if let Some(date) = billing.next(today).filter(|d| *d >= today) {
            // The cycle is named only when it is known.
            let cycle = billing
                .cycle
                .map(|c| format!("{} · ", c.label()))
                .unwrap_or_default();
            out.push(Event {
                kind: if billing.cancelled {
                    Kind::PlanEnds
                } else {
                    Kind::Renewal
                },
                key: key.clone(),
                at: Some(noon(date)),
                date_only: true,
                what: if billing.cancelled {
                    "plan ends"
                } else {
                    "plan renews"
                }
                .to_string(),
                note: match (billing.cancelled, billing.source) {
                    (true, _) => "cancelled: use it up before then".to_string(),
                    (false, Source::Api) => format!("{cycle}from the vendor"),
                    (false, Source::Manual) => format!("{cycle}set by hand"),
                },
            });
        }
        if let Some(at) = local
            .get(&key)
            .and_then(|l| l.login_expires)
            .filter(|t| *t > now && *t - now < Duration::days(60))
        {
            out.push(Event {
                kind: Kind::LoginExpires,
                key: key.clone(),
                at: Some(at),
                date_only: false,
                what: "sign-in expires".to_string(),
                note: "sign in again before then".to_string(),
            });
        }
    }
    out.sort_by_key(|r| r.at);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;
    use crate::model::Grant;
    use crate::store::{AccountMeta, Billing, Cycle};
    use crate::testutil::Sandbox;

    fn account(id: u32) -> Account {
        Account {
            harness: Harness::Claude,
            meta: AccountMeta {
                harness: "claude".into(),
                id,
                ..AccountMeta::default()
            },
        }
    }

    fn week(used: f64, reset_h: i64) -> Snapshot {
        Snapshot::new(vec![
            Window {
                kind: WindowKind::Session,
                label: "5h".into(),
                used_pct: 10.0,
                resets_at: Some(Utc::now() + Duration::hours(1)),
                window_secs: Some(18000),
            },
            Window {
                kind: WindowKind::Weekly,
                label: "week".into(),
                used_pct: used,
                resets_at: Some(Utc::now() + Duration::hours(reset_h)),
                window_secs: Some(604800),
            },
        ])
    }

    #[test]
    fn the_timeline_is_sorted_and_leaves_out_the_short_window() {
        let sb = Sandbox::new();
        let mut a = account(1);
        let mut b = account(2);
        sb.login(&a);
        sb.login(&b);
        a.meta.refresh.data = Some(week(40.0, 50));
        b.meta.refresh.data = Some(week(10.0, 5));
        b.meta.billing = Billing {
            cycle: Some(Cycle::Monthly),
            renews_on: Some(store::today() + Duration::days(1)),
            cancelled: false,
            source: Source::Manual,
        };
        let mut reg = Registry {
            accounts: vec![a, b],
        };
        let (logged_in, local) = facts(&mut reg);
        let events = upcoming(&reg, &logged_in, &local, Utc::now());
        let whats: Vec<(&str, &str)> = events
            .iter()
            .map(|e| (e.key.as_str(), e.what.as_str()))
            .collect();
        assert_eq!(
            whats,
            [
                ("claude-2", "week limit resets"),
                ("claude-2", "plan renews"),
                ("claude-1", "week limit resets"),
            ]
        );
        assert!(events[1].date_only);
        assert_eq!(events[1].countdown(Utc::now()), "1 day");
    }

    #[test]
    fn grants_sort_by_expiry_with_the_everlasting_last() {
        let _sb = Sandbox::new();
        let mut a = account(1);
        let grant = |days: Option<i64>| Grant {
            scope: "weekly".into(),
            count: 2,
            expires_at: days.map(|d| Utc::now() + Duration::days(d)),
            note: String::new(),
            source: Source::Manual,
            usable_now: None,
        };
        a.meta.grants = vec![grant(None), grant(Some(9)), grant(Some(2))];
        let reg = Registry { accounts: vec![a] };
        let out = grants(&reg, Utc::now());
        assert_eq!(out.len(), 3);
        assert!(out[0].at < out[1].at && out[2].at.is_none());
        assert_eq!(out[0].what, "2 × weekly reset");
        assert_eq!(grant_count(&reg, Utc::now()), 6);
    }

    #[test]
    fn state_reads_in_order_of_what_matters_most() {
        let _sb = Sandbox::new();
        let now = Utc::now();
        let mut a = account(1);
        assert_eq!(state(&a, false, 0, false, now), State::SignedOut);
        assert_eq!(state(&a, true, 0, false, now), State::Fetching);
        a.meta.refresh.data = Some(week(100.0, 50));
        assert_eq!(state(&a, true, 0, false, now), State::Blocked);
        assert_eq!(state(&a, true, 2, false, now), State::Live(2));
        a.meta.refresh.data = Some(week(30.0, 50));
        a.meta.refresh.error = Some("offline".into());
        assert_eq!(state(&a, true, 0, false, now), State::Stale);
    }

    #[test]
    fn the_launch_hint_prefers_the_alias() {
        let mut a = account(2);
        assert_eq!(launch_hint(&a, false), "claude --cswap=2");
        a.meta.label = "work".into();
        assert_eq!(launch_hint(&a, false), "claude --cswap=work");
        assert_eq!(launch_hint(&a, true), "claude");
        assert_eq!(launch_hint(&account(0), false), "claude --cswap=native");
    }

    #[test]
    fn credits_read_as_money_or_plain_units() {
        let usd = crate::model::Credits {
            enabled: true,
            used: Some(12.4),
            limit: Some(50.0),
            balance: None,
            unit: "USD".into(),
            resets_at: None,
        };
        assert_eq!(credits_text(&usd).unwrap(), "$12.40 of $50.00 spent");
        let off = crate::model::Credits::default();
        assert!(credits_text(&off).is_none());
        let balance = crate::model::Credits {
            enabled: true,
            balance: Some(310.0),
            unit: "credits".into(),
            ..crate::model::Credits::default()
        };
        assert_eq!(credits_text(&balance).unwrap(), "310 credits left");
    }
}
