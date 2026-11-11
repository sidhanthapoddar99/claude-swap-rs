//! Which account should the next session use?
//!
//! The idea is "use it or lose it": weekly quota that is still unused when the
//! week resets is gone, so the best account is the one that has the most left
//! per hour remaining. On top of that: skip what is blocked, avoid a nearly
//! full 5-hour window, and lean toward a plan that is about to end or a reset
//! that is about to expire.
//!
//! This reads only what is stored on disk. It never touches the network, so a
//! wrapped launch can call it on every start.

use chrono::{DateTime, Duration, Utc};

use crate::model::{self, WindowKind};
use crate::store::{self, Account, Settings};

#[derive(Debug, Clone)]
pub struct Pick {
    pub key: String,
    /// Can a session start on this account right now?
    pub usable: bool,
    pub score: f64,
    /// One line that says why it ranks where it does.
    pub headline: String,
    /// Everything that moved the score, for the detail view.
    pub reasons: Vec<String>,
}

/// Rank the accounts of one harness, best first. Unusable accounts come last,
/// soonest back first.
pub fn rank(accounts: &[&Account], settings: &Settings, now: DateTime<Utc>) -> Vec<Pick> {
    let mut picks: Vec<(Pick, Option<DateTime<Utc>>)> =
        accounts.iter().map(|a| judge(a, settings, now)).collect();
    picks.sort_by(|(a, a_back), (b, b_back)| {
        b.usable
            .cmp(&a.usable)
            .then(b.score.total_cmp(&a.score))
            .then(a_back.cmp(b_back))
            .then(a.key.cmp(&b.key))
    });
    picks.into_iter().map(|(p, _)| p).collect()
}

/// The account auto-pick would launch, if any is usable.
pub fn best(accounts: &[&Account], settings: &Settings, now: DateTime<Utc>) -> Option<Pick> {
    rank(accounts, settings, now).into_iter().find(|p| p.usable)
}

fn blocked(
    acc: &Account,
    headline: String,
    back: Option<DateTime<Utc>>,
) -> (Pick, Option<DateTime<Utc>>) {
    (
        Pick {
            key: acc.key(),
            usable: false,
            score: 0.0,
            reasons: vec![headline.clone()],
            headline,
        },
        back,
    )
}

fn judge(acc: &Account, settings: &Settings, now: DateTime<Utc>) -> (Pick, Option<DateTime<Utc>>) {
    let today = store::today();
    if !acc.harness.logged_in(&acc.config_dir()) {
        return blocked(acc, "not logged in".to_string(), None);
    }
    if acc.meta.billing.ended(today) {
        let day = acc.meta.billing.renews_on.expect("ended implies a date");
        return blocked(acc, format!("plan ended {}", day.format("%-d %b")), None);
    }

    let grants = acc.grants(now);
    let Some(snap) = acc.snapshot() else {
        let why = match &acc.meta.refresh.error {
            Some(e) => format!("no usage data ({e})"),
            None => "no usage data yet".to_string(),
        };
        return (
            Pick {
                key: acc.key(),
                usable: true,
                score: 30.0,
                reasons: vec![why.clone()],
                headline: why,
            },
            None,
        );
    };

    if let Some(w) = snap.blocking(now) {
        let back = w.resets_at;
        // A short window is never named anywhere in cswap: what matters is
        // that the account is out for now, and when it is back.
        let what = if crate::view::is_short(w) {
            "limit reached".to_string()
        } else {
            format!("{} limit reached", w.label)
        };
        let mut text = match back {
            Some(t) => format!("{what}, back {}", model::landing(t, settings.clock_24h)),
            None => what,
        };
        if !grants.is_empty() {
            text.push_str(" (a reset is available)");
        }
        return blocked(acc, text, back);
    }

    let mut score = 50.0;
    let mut reasons = Vec::new();
    let mut headline;

    if let Some(w) = snap.weekly() {
        let used = w.used_now(now);
        let left = 100.0 - used;
        if used >= settings.auto_skip_above_pct {
            return blocked(
                acc,
                format!("{} at {used:.0}%, nothing left to plan with", w.period()),
                w.resets_at,
            );
        }
        let window = w.window_secs.unwrap_or(7 * 86400) as f64 / 3600.0;
        let hours = match w.resets_at {
            Some(t) if t > now => ((t - now).num_minutes() as f64 / 60.0).max(1.0),
            // Rolled over, or no reset time: a whole window lies ahead.
            _ => window,
        };
        // Percent of quota that has to be spent per hour for none to go to
        // waste. An untouched week sits near 0.6; 60% left with 10 hours to
        // go is 6.
        let burn = left / hours;
        score += burn.min(10.0) * 8.0 + left * 0.2;
        headline = match w.resets_at {
            Some(t) if t > now => {
                format!(
                    "{left:.0}% of the {} left, resets in {}",
                    w.period(),
                    model::span(t - now)
                )
            }
            _ => format!("fresh {}, {left:.0}% left", w.period()),
        };
        if burn >= 2.0 && left >= 15.0 {
            reasons.push(format!("{headline}: use it or lose it"));
        } else {
            reasons.push(headline.clone());
        }
        if now - snap.fetched_at > Duration::hours(24) && !w.rolled_over(now) {
            reasons.push(format!(
                "usage is {} old",
                model::span(now - snap.fetched_at)
            ));
            score -= 5.0;
        }
    } else {
        headline = "no weekly limit reported".to_string();
        reasons.push(headline.clone());
    }

    if let Some(s) = snap.session() {
        let used = s.used_now(now);
        if used >= 85.0 {
            score -= 40.0;
            let when = s
                .resets_at
                .filter(|t| *t > now)
                .map(|t| format!(" for the next {}", model::span(t - now)))
                .unwrap_or_default();
            let text = format!("close to being rate limited{when}");
            headline = format!("{headline}; {text}");
            reasons.push(text);
        }
    }

    // A scoped weekly limit (one model) that is exhausted does not block the
    // account, but it is worth saying.
    for w in snap
        .windows
        .iter()
        .filter(|w| w.kind == WindowKind::WeeklyScoped)
    {
        if w.used_now(now) >= 99.5 {
            score -= 10.0;
            reasons.push(format!("{} weekly limit reached", w.label));
        }
    }

    if acc.meta.billing.cancelled {
        if let Some(end) = acc.meta.billing.renews_on {
            let days = (end - today).num_days();
            if (0..=10).contains(&days) {
                score += 25.0 - days as f64;
                reasons.push(format!("plan ends in {days}d: spend it while it lasts"));
            }
        }
    }

    if let Some(soonest) = grants.iter().filter_map(|g| g.expires_at).min() {
        if soonest - now <= Duration::days(3) {
            score += 15.0;
            reasons.push(format!(
                "a reset expires in {}: run this one down, then redeem it",
                model::span(soonest - now)
            ));
        }
    }

    (
        Pick {
            key: acc.key(),
            usable: true,
            score,
            headline,
            reasons,
        },
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;
    use crate::model::{Grant, Snapshot, Source, Window};
    use crate::store::AccountMeta;
    use crate::testutil::Sandbox;

    fn account(sb: &Sandbox, id: u32, weekly: f64, reset_h: i64, session: f64) -> Account {
        let mut acc = Account {
            harness: Harness::Claude,
            meta: AccountMeta {
                harness: "claude".into(),
                id,
                ..AccountMeta::default()
            },
        };
        sb.login(&acc);
        let now = Utc::now();
        acc.meta.refresh.data = Some(Snapshot::new(vec![
            Window {
                kind: WindowKind::Session,
                label: "5h".into(),
                used_pct: session,
                resets_at: Some(now + Duration::hours(2)),
                window_secs: Some(18000),
            },
            Window {
                kind: WindowKind::Weekly,
                label: "week".into(),
                used_pct: weekly,
                resets_at: Some(now + Duration::hours(reset_h)),
                window_secs: Some(604800),
            },
        ]));
        acc
    }

    fn order(accs: &[&Account]) -> Vec<String> {
        rank(accs, &Settings::default(), Utc::now())
            .into_iter()
            .map(|p| p.key)
            .collect()
    }

    #[test]
    fn the_account_about_to_reset_with_quota_left_goes_first() {
        let sb = Sandbox::new();
        let fresh = account(&sb, 1, 5.0, 160, 0.0); // whole week ahead
        let expiring = account(&sb, 2, 40.0, 8, 0.0); // 60% left, 8h to go
        assert_eq!(order(&[&fresh, &expiring]), ["claude-2", "claude-1"]);
    }

    #[test]
    fn a_blocked_account_is_never_picked_and_sorts_by_return_time() {
        let sb = Sandbox::new();
        let spent_late = account(&sb, 1, 100.0, 90, 0.0);
        let spent_soon = account(&sb, 2, 100.0, 3, 0.0);
        let ok = account(&sb, 3, 80.0, 100, 0.0);
        let accs = [&spent_late, &spent_soon, &ok];
        assert_eq!(order(&accs), ["claude-3", "claude-2", "claude-1"]);
        let best = best(&accs, &Settings::default(), Utc::now()).unwrap();
        assert_eq!(best.key, "claude-3");
    }

    #[test]
    fn a_full_session_window_drops_an_account_below_a_free_one() {
        let sb = Sandbox::new();
        let busy = account(&sb, 1, 30.0, 20, 95.0);
        let free = account(&sb, 2, 50.0, 100, 0.0);
        assert_eq!(order(&[&busy, &free]), ["claude-2", "claude-1"]);
    }

    #[test]
    fn a_short_window_counts_but_is_never_named() {
        let sb = Sandbox::new();
        let nearly = account(&sb, 1, 30.0, 20, 95.0);
        let full = account(&sb, 2, 30.0, 20, 100.0);
        let picks = rank(&[&nearly, &full], &Settings::default(), Utc::now());
        // Nearly full still runs, with a word on why it ranks low.
        assert!(picks[0].usable && picks[0].key == "claude-1");
        assert!(
            picks[0].reasons.iter().any(|r| r.contains("rate limited")),
            "{:?}",
            picks[0].reasons
        );
        // Full is out until the window comes round.
        assert!(!picks[1].usable);
        assert!(
            picks[1].headline.starts_with("limit reached, back"),
            "{}",
            picks[1].headline
        );
        for p in &picks {
            let all = format!("{} {}", p.headline, p.reasons.join(" "));
            assert!(!all.contains("5h") && !all.contains("window"), "{all}");
        }
    }

    #[test]
    fn a_weekly_window_past_its_reset_counts_as_fresh() {
        let sb = Sandbox::new();
        let rolled = account(&sb, 1, 100.0, -1, 0.0);
        let p = &rank(&[&rolled], &Settings::default(), Utc::now())[0];
        assert!(p.usable, "{p:?}");
        assert!(p.headline.contains("fresh"), "{}", p.headline);
    }

    #[test]
    fn logged_out_accounts_and_ended_plans_are_unusable() {
        let sb = Sandbox::new();
        let mut gone = account(&sb, 1, 10.0, 50, 0.0);
        std::fs::remove_file(gone.config_dir().join(".credentials.json")).unwrap();
        assert!(!rank(&[&gone], &Settings::default(), Utc::now())[0].usable);

        sb.login(&gone);
        gone.meta.billing.cancelled = true;
        gone.meta.billing.renews_on = Some(store::today() - Duration::days(1));
        let p = &rank(&[&gone], &Settings::default(), Utc::now())[0];
        assert!(!p.usable && p.headline.contains("plan ended"), "{p:?}");
    }

    #[test]
    fn an_expiring_reset_grant_pulls_an_account_forward() {
        let sb = Sandbox::new();
        let plain = account(&sb, 1, 50.0, 80, 0.0);
        let mut granted = account(&sb, 2, 50.0, 80, 0.0);
        granted.meta.grants.push(Grant {
            scope: "weekly".into(),
            count: 1,
            expires_at: Some(Utc::now() + Duration::hours(30)),
            note: String::new(),
            source: Source::Manual,
            usable_now: None,
        });
        assert_eq!(order(&[&plain, &granted]), ["claude-2", "claude-1"]);
    }
}
