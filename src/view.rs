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
        return Some((format!("the {} has only just started", w.period()), false));
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
            w.period()
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

// -------------------------------------------------------------------- resets

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A spare reset that can still be spent. Its time is when it expires.
    Spare,
    /// One or more limits of an account going back to zero at the same moment.
    Limit,
    Renewal,
    PlanEnds,
}

/// One line of the resets view.
#[derive(Debug, Clone)]
pub struct Event {
    pub kind: Kind,
    pub key: String,
    /// When it happens; for a spare reset, when it expires.
    pub at: Option<DateTime<Utc>>,
    /// True when only the day is known.
    pub date_only: bool,
    /// The type column: `weekly`, `monthly`, `spare`, `renews`, `ends`.
    pub tag: &'static str,
    /// What resets: `week + Fable`, `all limits`. For a renewal, the plan.
    pub what: String,
    pub note: String,
    /// For a limit: how full the fullest of its windows is.
    pub used: Option<f64>,
}

impl Event {
    /// The exact date. The time of day is given only within two days, where
    /// it matters; further out the day is enough.
    pub fn date(&self, now: DateTime<Utc>, clock_24h: bool) -> String {
        match self.at {
            None => "never".to_string(),
            Some(at) if self.date_only || at - now > Duration::days(2) => at
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
/// come last. Resets of one account that are alike in every way are one line
/// with a count.
pub fn grants(reg: &Registry, now: DateTime<Utc>) -> Vec<Event> {
    let mut out = Vec::new();
    for acc in &reg.accounts {
        let mut merged: Vec<crate::model::Grant> = Vec::new();
        for g in acc.grants(now) {
            match merged.iter_mut().find(|m| {
                m.scope == g.scope
                    && m.expires_at == g.expires_at
                    && m.source == g.source
                    && m.usable_now == g.usable_now
            }) {
                Some(m) => m.count += g.count,
                None => merged.push(g),
            }
        }
        for g in merged {
            let mut note = match g.source {
                Source::Api => format!("from {}", acc.harness.spec().vendor),
                Source::Manual => "noted by hand".to_string(),
            };
            match g.usable_now {
                Some(true) => note.push_str(" · spendable now"),
                Some(false) => note.push_str(" · not spendable yet"),
                None => {}
            }
            out.push(Event {
                kind: Kind::Spare,
                key: acc.key(),
                at: g.expires_at,
                date_only: false,
                tag: "spare",
                what: if g.count == 1 {
                    g.scope.clone()
                } else {
                    format!("{} × {}", g.count, g.scope)
                },
                note,
                used: None,
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

/// A limit worth a line: the weekly and monthly ones. The short windows come
/// round too often.
fn long_window(w: &Window) -> bool {
    match w.kind {
        WindowKind::Weekly | WindowKind::WeeklyScoped | WindowKind::Monthly => true,
        WindowKind::Other => w.window_secs.is_some_and(|s| s > 36 * 3600),
        WindowKind::Session | WindowKind::Daily => false,
    }
}

/// When each signed-in account's limits go back to zero, soonest first.
/// Limits of one account that reset at the same moment are one line: Claude's
/// week and its per-model week reset together, so they read `week + Fable`.
pub fn limit_resets(reg: &Registry, logged_in: &HashSet<String>, now: DateTime<Utc>) -> Vec<Event> {
    let mut out = Vec::new();
    for acc in &reg.accounts {
        let key = acc.key();
        let Some(snap) = acc.snapshot().filter(|_| logged_in.contains(&key)) else {
            continue;
        };
        // Grouped by the minute, in the order the windows are listed.
        let mut groups: Vec<(DateTime<Utc>, Vec<&Window>)> = Vec::new();
        for w in snap.windows.iter().filter(|w| long_window(w)) {
            let Some(at) = w.resets_at.filter(|t| *t > now) else {
                continue;
            };
            match groups
                .iter_mut()
                .find(|(t, _)| (*t - at).num_seconds().abs() < 60)
            {
                Some((_, ws)) => ws.push(w),
                None => groups.push((at, vec![w])),
            }
        }
        for (at, ws) in groups {
            let labels: Vec<&str> = ws.iter().map(|w| w.label.as_str()).collect();
            let note = if ws.len() == 1 {
                format!("{:.0}% used", ws[0].used_now(now))
            } else {
                ws.iter()
                    .map(|w| format!("{} {:.0}%", w.label, w.used_now(now)))
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            out.push(Event {
                kind: Kind::Limit,
                key: key.clone(),
                at: Some(at),
                date_only: false,
                tag: if ws.iter().all(|w| w.kind == WindowKind::Monthly) {
                    "monthly"
                } else {
                    "weekly"
                },
                what: labels.join(" + "),
                note,
                used: ws.iter().map(|w| w.used_now(now)).reduce(f64::max),
            });
        }
    }
    out.sort_by_key(|r| r.at);
    out
}

/// The combined view: every limit reset and every spare reset that expires,
/// soonest first. Nothing else is in it.
pub fn coming_up(reg: &Registry, logged_in: &HashSet<String>, now: DateTime<Utc>) -> Vec<Event> {
    let mut out = limit_resets(reg, logged_in, now);
    out.extend(grants(reg, now).into_iter().filter(|g| g.at.is_some()));
    out.sort_by_key(|r| r.at);
    out
}

/// When each plan renews or ends, soonest first.
pub fn renewals(reg: &Registry) -> Vec<Event> {
    let today = store::today();
    let mut out = Vec::new();
    for acc in &reg.accounts {
        let billing = &acc.meta.billing;
        let Some(date) = billing.next(today).filter(|d| *d >= today) else {
            continue;
        };
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
            key: acc.key(),
            at: Some(noon(date)),
            date_only: true,
            tag: if billing.cancelled { "ends" } else { "renews" },
            what: acc.meta.plan.clone().unwrap_or_default(),
            note: match (billing.cancelled, billing.source) {
                (true, _) => "cancelled: use it up before then".to_string(),
                (false, Source::Api) => format!("{cycle}from the vendor"),
                (false, Source::Manual) => format!("{cycle}set by hand"),
            },
            used: None,
        });
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
    fn limits_that_reset_together_are_one_line_and_the_short_window_is_left_out() {
        let sb = Sandbox::new();
        let mut a = account(1);
        let mut b = account(2);
        sb.login(&a);
        sb.login(&b);
        let now = Utc::now();
        let mut snap = week(40.0, 50);
        let at = snap.windows[1].resets_at;
        snap.windows.push(Window {
            kind: WindowKind::WeeklyScoped,
            label: "Fable".into(),
            used_pct: 70.0,
            resets_at: at,
            window_secs: Some(604800),
        });
        // A second group's short window, as Antigravity reports: never a line.
        snap.windows.push(Window {
            kind: WindowKind::Other,
            label: "other 5h".into(),
            used_pct: 0.0,
            resets_at: Some(now + Duration::hours(3)),
            window_secs: Some(18000),
        });
        a.meta.refresh.data = Some(snap);
        b.meta.refresh.data = Some(week(10.0, 5));
        let mut reg = Registry {
            accounts: vec![a, b],
        };
        let (logged_in, _) = facts(&mut reg);
        let events = limit_resets(&reg, &logged_in, now);
        let lines: Vec<(&str, &str, &str)> = events
            .iter()
            .map(|e| (e.key.as_str(), e.tag, e.what.as_str()))
            .collect();
        assert_eq!(
            lines,
            [
                ("claude-2", "weekly", "week"),
                ("claude-1", "weekly", "week + Fable"),
            ]
        );
        assert_eq!(events[1].note, "week 40% · Fable 70%");
        assert_eq!(events[1].used.map(f64::round), Some(70.0));
        assert_eq!(events[0].note, "10% used");
    }

    #[test]
    fn coming_up_holds_resets_only_and_renewals_stand_apart() {
        let sb = Sandbox::new();
        let mut a = account(1);
        sb.login(&a);
        let now = Utc::now();
        a.meta.refresh.data = Some(week(40.0, 50));
        a.meta.plan = Some("Max".into());
        a.meta.billing = Billing {
            cycle: Some(Cycle::Monthly),
            renews_on: Some(store::today() + Duration::days(1)),
            cancelled: false,
            source: Source::Manual,
        };
        let grant = |days: Option<i64>| Grant {
            scope: "week".into(),
            count: 1,
            expires_at: days.map(|d| now + Duration::days(d)),
            note: String::new(),
            source: Source::Manual,
            usable_now: None,
        };
        a.meta.grants = vec![grant(Some(1)), grant(None)];
        let mut reg = Registry { accounts: vec![a] };
        let (logged_in, _) = facts(&mut reg);

        // The spare reset that expires and the weekly reset, by time. The
        // spare reset that never expires has no date, so it is not in here.
        let coming = coming_up(&reg, &logged_in, now);
        let tags: Vec<&str> = coming.iter().map(|e| e.tag).collect();
        assert_eq!(tags, ["spare", "weekly"]);

        let renew = renewals(&reg);
        assert_eq!(renew.len(), 1);
        assert_eq!((renew[0].tag, renew[0].what.as_str()), ("renews", "Max"));
        assert_eq!(renew[0].note, "monthly · set by hand");
        assert_eq!(renew[0].countdown(now), "1 day");
    }

    #[test]
    fn the_time_of_day_is_given_only_within_two_days() {
        let now = Utc::now();
        let at = |hours: i64| Event {
            kind: Kind::Limit,
            key: "claude-1".into(),
            at: Some(now + Duration::hours(hours)),
            date_only: false,
            tag: "weekly",
            what: "week".into(),
            note: String::new(),
            used: None,
        };
        assert!(at(20).date(now, true).contains(':'));
        assert!(at(47).date(now, true).contains(':'));
        assert!(!at(49).date(now, true).contains(':'));
        assert!(!at(24 * 9).date(now, true).contains(':'));
    }

    #[test]
    fn grants_sort_by_expiry_and_alike_ones_are_counted_together() {
        let _sb = Sandbox::new();
        let mut a = account(1);
        let now = Utc::now();
        let late = now + Duration::days(9);
        let grant = |at: Option<DateTime<Utc>>| Grant {
            scope: "weekly".into(),
            count: 2,
            expires_at: at,
            note: String::new(),
            source: Source::Manual,
            usable_now: None,
        };
        a.meta.grants = vec![
            grant(None),
            grant(Some(late)),
            grant(Some(now + Duration::days(2))),
            grant(Some(late)),
        ];
        let reg = Registry { accounts: vec![a] };
        let out = grants(&reg, now);
        assert_eq!(out.len(), 3);
        assert!(out[0].at < out[1].at && out[2].at.is_none());
        assert_eq!(out[0].what, "2 × weekly");
        assert_eq!(out[1].what, "4 × weekly");
        assert_eq!(grant_count(&reg, now), 8);
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
