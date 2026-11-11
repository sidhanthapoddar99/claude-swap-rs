//! What the dashboard and the text commands both show, worked out once: the
//! state of an account in a word, its plan, and the timeline of resets.

use chrono::{DateTime, Duration, TimeZone, Utc};
use std::collections::{HashMap, HashSet};

use crate::harness::Local;
use crate::model::{self, Snapshot, Window, WindowKind};
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
    match hs.start.as_str() {
        "default" => acc.is_native(),
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
    let sel = if acc.is_native() {
        "default".to_string()
    } else if !acc.meta.label.is_empty() {
        acc.meta.label.clone()
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
        .or_else(|| snap.windows.iter().find(|w| !is_short(w)))
}

/// A window that comes round within hours. cswap keeps these to know when an
/// account is blocked, and never shows them.
pub fn is_short(w: &Window) -> bool {
    w.kind == WindowKind::Session || w.window_secs.is_some_and(|s| s <= 12 * 3600)
}

/// The limits to show for an account: the weekly one first, then the rest.
/// The short windows are left out.
pub fn usage_windows(snap: &Snapshot) -> Vec<&Window> {
    let mut out: Vec<&Window> = snap.windows.iter().filter(|w| !is_short(w)).collect();
    out.sort_by_key(|w| match w.kind {
        WindowKind::Weekly => 0,
        WindowKind::Monthly => 1,
        WindowKind::WeeklyScoped => 2,
        _ => 3,
    });
    out
}

/// What state an account is in, in one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    SignedOut,
    Fetching,
    Blocked,
    Stale,
    Ready,
}

impl State {
    pub fn label(self) -> String {
        match self {
            State::SignedOut => "signed out".to_string(),
            State::Fetching => "waiting".to_string(),
            State::Blocked => "limit hit".to_string(),
            State::Stale => "stale".to_string(),
            State::Ready => "ready".to_string(),
        }
    }
}

/// `fetching` is true while a refresh for the account is in flight.
pub fn state(acc: &Account, signed_in: bool, fetching: bool, now: DateTime<Utc>) -> State {
    if !signed_in {
        return State::SignedOut;
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
                model::landing(runs_out, clock_24h),
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

/// A short warning when the limit will be hit before it resets, and nothing
/// otherwise: the bar and its pace marker already show the rest.
pub fn pace_warning(w: &Window, now: DateTime<Utc>) -> Option<String> {
    let used = w.used_now(now);
    let (reset, secs) = (w.resets_at?, w.window_secs?);
    if used >= 99.5 || used <= 0.0 || w.elapsed_pct(now)? < 5.0 {
        return None;
    }
    if w.projected_pct(now)? < 100.0 {
        return None;
    }
    let start = reset - Duration::seconds(secs);
    let elapsed_secs = (now - start).num_seconds() as f64;
    let runs_out = start + Duration::seconds((elapsed_secs * 100.0 / used) as i64);
    Some(format!("runs out {} early", model::until(reset, runs_out)))
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
    /// An account's weekly (or monthly) limit going back to zero.
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
    /// What resets: `weekly`, `monthly`, `2 × Fable`. For a renewal, the plan.
    pub what: String,
    /// For a renewal: its cycle, or that the plan ends. Empty otherwise.
    pub note: String,
    /// How full the account's weekly limit is. One number for the account,
    /// never split by model.
    pub used: Option<f64>,
    /// How much of that week has passed, for the pace marker on the bar.
    pub pace: Option<f64>,
}

impl Event {
    /// The exact date. The time of day is given only within two days, where
    /// it matters; further out the day is enough.
    pub fn date(&self, now: DateTime<Utc>, clock_24h: bool) -> String {
        match self.at {
            None => "never".to_string(),
            Some(at) if self.date_only || at - now > Duration::days(2) => model::day(at),
            Some(at) => model::landing(at, clock_24h),
        }
    }

    /// How long until then, in the form used for every countdown: `3d`,
    /// `1d 6h`, `4h 25m`. Whole days when only the day is known.
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
            Some(at) => model::until(at, now),
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

/// What a spare reset is called. A vendor lists every limit one clears, such
/// as `5h + week + Fable`. What matters is the widest of them: a reset that
/// clears the week is a weekly reset, whatever else goes with it. One that
/// clears a single model's limit keeps that model's name.
pub fn reset_name(scope: &str) -> String {
    let parts: Vec<&str> = scope
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let has = |words: &[&str]| {
        parts
            .iter()
            .any(|p| words.contains(&p.to_lowercase().as_str()))
    };
    if has(&["month", "monthly"]) {
        return "monthly".to_string();
    }
    if has(&["week", "weekly", "all limits", "limits", "all"]) {
        return "weekly".to_string();
    }
    // The short windows are never named.
    let rest: Vec<&str> = parts
        .into_iter()
        .filter(|p| !matches!(p.to_lowercase().as_str(), "5h" | "session" | "5-hour"))
        .collect();
    if rest.is_empty() {
        "rate limit".to_string()
    } else {
        rest.join(" + ")
    }
}

/// How full an account's weekly limit is, and how much of the week has passed.
fn week_of(acc: &Account, now: DateTime<Utc>) -> (Option<f64>, Option<f64>) {
    match acc.snapshot().and_then(main_window) {
        Some(w) => (Some(w.used_now(now)), w.elapsed_pct(now)),
        None => (None, None),
    }
}

/// Every spare reset, soonest to expire first; the ones that never expire
/// come last. Resets of one account with the same name and the same expiry
/// are one line with a count.
pub fn grants(reg: &Registry, now: DateTime<Utc>) -> Vec<Event> {
    let mut out = Vec::new();
    for acc in reg.profiles() {
        let mut merged: Vec<(String, Option<DateTime<Utc>>, u32)> = Vec::new();
        for g in acc.grants(now) {
            let name = reset_name(&g.scope);
            match merged
                .iter_mut()
                .find(|(n, at, _)| *n == name && *at == g.expires_at)
            {
                Some(m) => m.2 += g.count,
                None => merged.push((name, g.expires_at, g.count)),
            }
        }
        let (used, pace) = week_of(acc, now);
        for (name, at, count) in merged {
            out.push(Event {
                kind: Kind::Spare,
                key: acc.key(),
                at,
                date_only: false,
                what: if count == 1 {
                    name
                } else {
                    format!("{count} × {name}")
                },
                note: String::new(),
                used,
                pace,
            });
        }
    }
    out.sort_by_key(|r| (r.at.is_none(), r.at));
    out
}

/// How many spare resets all accounts hold together.
pub fn grant_count(reg: &Registry, now: DateTime<Utc>) -> u32 {
    reg.profiles()
        .flat_map(|a| a.grants(now))
        .map(|g| g.count)
        .sum()
}

/// When each signed-in account's weekly limit goes back to zero, soonest
/// first. One line per account: the limit the account is judged by, and
/// none of the per-model limits beside it.
pub fn limit_resets(reg: &Registry, logged_in: &HashSet<String>, now: DateTime<Utc>) -> Vec<Event> {
    let mut out = Vec::new();
    for acc in reg.profiles() {
        let key = acc.key();
        let Some(w) = acc
            .snapshot()
            .filter(|_| logged_in.contains(&key))
            .and_then(main_window)
        else {
            continue;
        };
        let Some(at) = w.resets_at.filter(|t| *t > now) else {
            continue;
        };
        out.push(Event {
            kind: Kind::Limit,
            key,
            at: Some(at),
            date_only: false,
            what: match w.kind {
                WindowKind::Weekly => "weekly".to_string(),
                WindowKind::Monthly => "monthly".to_string(),
                _ => w.label.clone(),
            },
            note: String::new(),
            used: Some(w.used_now(now)),
            pace: w.elapsed_pct(now),
        });
    }
    out.sort_by_key(|r| r.at);
    out
}

/// When each plan renews or ends, soonest first.
pub fn renewals(reg: &Registry) -> Vec<Event> {
    let today = store::today();
    let mut out = Vec::new();
    for acc in reg.profiles() {
        let billing = &acc.meta.billing;
        let Some(date) = billing.next(today).filter(|d| *d >= today) else {
            continue;
        };
        out.push(Event {
            kind: if billing.cancelled {
                Kind::PlanEnds
            } else {
                Kind::Renewal
            },
            key: acc.key(),
            at: Some(noon(date)),
            date_only: true,
            what: acc.meta.plan.clone().unwrap_or_default(),
            // The cycle is named only when it is known.
            note: if billing.cancelled {
                "ends · cancelled".to_string()
            } else {
                billing
                    .cycle
                    .map(|c| c.label().to_string())
                    .unwrap_or_default()
            },
            used: None,
            pace: None,
        });
    }
    out.sort_by_key(|r| r.at);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;
    use crate::model::{Grant, Source};
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
    fn an_account_has_one_weekly_line_with_the_weeks_own_usage() {
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
        // One line per account, soonest first. A model's limit that resets
        // with the week is not named, and neither is the short window.
        let lines: Vec<(&str, &str)> = events
            .iter()
            .map(|e| (e.key.as_str(), e.what.as_str()))
            .collect();
        assert_eq!(lines, [("claude-2", "weekly"), ("claude-1", "weekly")]);
        // The usage is the week's own figure, never a model's.
        assert_eq!(events[1].used.map(f64::round), Some(40.0));
        assert_eq!(events[0].used.map(f64::round), Some(10.0));
        assert!(events.iter().all(|e| e.note.is_empty() && e.pace.is_some()));
    }

    #[test]
    fn spare_resets_weekly_resets_and_renewals_are_three_lists() {
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

        // Two spare resets: the one that expires first, the one that never
        // does last. Each carries the account's weekly usage.
        let spare = grants(&reg, now);
        assert_eq!(spare.len(), 2);
        assert!(spare[0].at.is_some() && spare[1].at.is_none());
        assert!(spare
            .iter()
            .all(|e| e.kind == Kind::Spare && e.what == "weekly"));
        assert!(spare.iter().all(|e| e.used.map(f64::round) == Some(40.0)));

        let weekly = limit_resets(&reg, &logged_in, now);
        assert_eq!(weekly.len(), 1);
        assert_eq!(
            (weekly[0].kind, weekly[0].what.as_str()),
            (Kind::Limit, "weekly")
        );

        let renew = renewals(&reg);
        assert_eq!(renew.len(), 1);
        assert_eq!(
            (renew[0].kind, renew[0].what.as_str()),
            (Kind::Renewal, "Max")
        );
        assert_eq!(renew[0].note, "monthly");
        assert_eq!(renew[0].used, None);
        assert_eq!(renew[0].countdown(now), "1 day");
    }

    #[test]
    fn a_spare_reset_is_named_after_the_widest_limit_it_clears() {
        // Whatever else goes with it, a reset that clears the week is weekly.
        for scope in [
            "week",
            "weekly",
            "5h + week + Fable",
            "week + Fable",
            "all limits",
        ] {
            assert_eq!(reset_name(scope), "weekly", "{scope}");
        }
        assert_eq!(reset_name("month + 5h"), "monthly");
        // One model's reset keeps the model's name; a short window has none.
        assert_eq!(reset_name("Fable"), "Fable");
        assert_eq!(reset_name("5h + Fable"), "Fable");
        assert_eq!(reset_name("5h"), "rate limit");
        assert_eq!(reset_name(""), "rate limit");
    }

    #[test]
    fn a_date_is_always_a_date_and_never_a_word() {
        let now = Utc::now();
        for hours in [0, 1, 20, 30, 47, 49, 200] {
            let at = now + Duration::hours(hours);
            for text in [
                model::day(at),
                model::landing(at, true),
                model::landing(at, false),
            ] {
                let lower = text.to_lowercase();
                assert!(
                    !lower.contains("today")
                        && !lower.contains("tomorrow")
                        && !lower.contains("tmrw"),
                    "{text}"
                );
                // A weekday, a day of the month and a month, in that order.
                let parts: Vec<&str> = text.split(' ').collect();
                assert!(
                    parts.len() >= 3 && parts[1].parse::<u32>().is_ok(),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn the_time_of_day_is_given_only_within_two_days() {
        let now = Utc::now();
        let at = |hours: i64| Event {
            kind: Kind::Limit,
            key: "claude-1".into(),
            at: Some(now + Duration::hours(hours)),
            date_only: false,
            what: "weekly".into(),
            note: String::new(),
            used: None,
            pace: None,
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
        assert_eq!(state(&a, false, false, now), State::SignedOut);
        assert_eq!(state(&a, true, false, now), State::Fetching);
        a.meta.refresh.data = Some(week(100.0, 50));
        assert_eq!(state(&a, true, false, now), State::Blocked);
        a.meta.refresh.data = Some(week(30.0, 50));
        assert_eq!(state(&a, true, false, now), State::Ready);
        // Old numbers stay on screen while new ones are fetched.
        assert_eq!(state(&a, true, true, now), State::Ready);
        a.meta.refresh.error = Some("offline".into());
        assert_eq!(state(&a, true, false, now), State::Stale);
    }

    #[test]
    fn the_short_windows_are_never_among_the_limits_shown() {
        let mut snap = week(40.0, 50);
        snap.windows.push(Window {
            kind: WindowKind::WeeklyScoped,
            label: "Fable".into(),
            used_pct: 70.0,
            resets_at: snap.windows[1].resets_at,
            window_secs: Some(604800),
        });
        snap.windows.push(Window {
            kind: WindowKind::Other,
            label: "other 5h".into(),
            used_pct: 99.0,
            resets_at: None,
            window_secs: Some(18000),
        });
        let labels: Vec<&str> = usage_windows(&snap)
            .iter()
            .map(|w| w.label.as_str())
            .collect();
        assert_eq!(labels, ["week", "Fable"]);
        assert_eq!(main_window(&snap).unwrap().label, "week");
        // An account with nothing but a short window has no limit to show.
        snap.windows.truncate(1);
        assert!(usage_windows(&snap).is_empty() && main_window(&snap).is_none());
    }

    #[test]
    fn a_warning_is_given_only_when_the_limit_runs_out_before_the_reset() {
        let now = Utc::now();
        let win = |used: f64, reset_h: i64| Window {
            kind: WindowKind::Weekly,
            label: "week".into(),
            used_pct: used,
            resets_at: Some(now + Duration::hours(reset_h)),
            window_secs: Some(604800),
        };
        // Half the week gone and 40% used: on pace, nothing to say.
        assert_eq!(pace_warning(&win(40.0, 84), now), None);
        // Half the week gone and 80% used: out with a good day to spare.
        let text = pace_warning(&win(80.0, 84), now).unwrap();
        assert!(
            text.starts_with("runs out ") && text.ends_with(" early"),
            "{text}"
        );
        // Already used up, or only just started: the bar says it.
        assert_eq!(pace_warning(&win(100.0, 84), now), None);
        assert_eq!(pace_warning(&win(3.0, 167), now), None);
    }

    #[test]
    fn a_bare_launch_starts_on_the_default_login_an_account_or_the_best_one() {
        let hs = |start: &str| HarnessSettings {
            start: start.to_string(),
            ..HarnessSettings::default()
        };
        assert!(is_default(&account(0), &hs("default")));
        assert!(!is_default(&account(1), &hs("default")));
        assert!(is_default(&account(2), &hs("2")));
        assert!(!is_default(&account(0), &hs("2")));
        assert!(!is_default(&account(0), &hs("auto")) && !is_default(&account(2), &hs("auto")));
    }

    #[test]
    fn the_launch_hint_uses_the_name() {
        let mut a = account(2);
        assert_eq!(launch_hint(&a, false), "claude --cswap=2");
        a.meta.label = "work".into();
        assert_eq!(launch_hint(&a, false), "claude --cswap=work");
        assert_eq!(launch_hint(&a, true), "claude");
        assert_eq!(launch_hint(&account(0), false), "claude --cswap=default");
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
