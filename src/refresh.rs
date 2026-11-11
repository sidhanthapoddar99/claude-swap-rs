//! Fetch one account's usage and store the outcome in its settings.
//!
//! Only the open dashboard and the explicit `cswap refresh` call this. A
//! wrapped launch never does: it reads what was last stored.
//!
//! The dashboard asks in rounds, one per refresh interval, and asks each
//! login once: a root login that an account also holds takes that account's
//! answer. Two dashboards open at once share the rounds, because a fetch is
//! claimed on disk before it is made.

use chrono::{DateTime, Duration, Utc};
use std::time::SystemTime;

use crate::harness::Local;
use crate::history;
use crate::http::FetchError;
use crate::manage;
use crate::model::Source;
use crate::store::{Account, AccountMeta, Registry, Settings};

/// Fold what a login says about itself into the stored account. A field the
/// login does not report keeps its stored value, and anything entered by hand
/// is left alone.
pub fn merge(meta: &mut AccountMeta, local: &Local) {
    let id = &mut meta.identity;
    if local.identity.email.is_some() {
        // A different login: the usage stored so far is someone else's.
        if id.email.is_some() && id.email != local.identity.email {
            meta.refresh.data = None;
        }
        id.email = local.identity.email.clone();
    }
    if local.identity.account_id.is_some() {
        id.account_id = local.identity.account_id.clone();
    }
    if local.identity.org.is_some() {
        id.org = local.identity.org.clone();
    }
    if local.identity.name.is_some() {
        id.name = local.identity.name.clone();
    }
    if local.plan.is_some() && (meta.plan_source == Source::Api || meta.plan.is_none()) {
        meta.plan = local.plan.clone();
        meta.plan_source = Source::Api;
    }
    if let Some(b) = &local.billing {
        if meta.billing.source == Source::Api || meta.billing.renews_on.is_none() {
            // A hint that names only the cycle must not wipe a known date.
            let date = b.renews_on.or(meta.billing.renews_on);
            meta.billing = b.clone();
            meta.billing.renews_on = date;
        }
    }
}

/// Update who the account is from disk alone. No network.
pub fn refresh_local(acc: &mut Account) -> anyhow::Result<()> {
    let local = acc.harness.local(&acc.config_dir(), acc.is_native());
    let logged_in = acc.harness.logged_in(&acc.config_dir());
    // Nothing on disk yet for a native login nobody has stored anything about:
    // do not create its settings file just to say so.
    if acc.is_native() && !logged_in && !acc.dir().exists() {
        return Ok(());
    }
    acc.update(|m| merge(m, &local))
}

/// Fetch usage for one account and record the outcome, success or not.
pub fn refresh_account(acc: &mut Account) -> Result<(), FetchError> {
    let native = acc.is_native();
    let dir = acc.config_dir();
    let result = acc.harness.fetch(&dir, native, acc.snapshot());
    let now = Utc::now();
    let stored = match &result {
        Ok(fetched) => {
            history::record(&acc.key(), &fetched.snapshot);
            acc.update(|m| {
                merge(m, &fetched.local);
                m.refresh.last_attempt = Some(now);
                m.refresh.last_success = Some(now);
                m.refresh.error = None;
                m.refresh.needs_user = false;
                m.refresh.retry_at = None;
                m.refresh.data = Some(fetched.snapshot.clone());
            })
        }
        Err(e) => {
            let local = acc.harness.local(&dir, native);
            acc.update(|m| {
                merge(m, &local);
                m.refresh.last_attempt = Some(now);
                m.refresh.error = Some(e.text.clone());
                m.refresh.needs_user = e.needs_user();
                // The vendors' own clients wait five minutes to an hour after
                // a rate limit. Half an hour sits inside that.
                m.refresh.retry_at =
                    (e.why == crate::http::Why::RateLimited).then(|| now + Duration::minutes(30));
            })
        }
    };
    acc.harness.harden(&dir);
    match (result, stored) {
        (Err(e), _) => Err(e),
        (Ok(_), Err(e)) => Err(FetchError::new(
            crate::http::Why::Shape,
            format!("fetched, but could not save: {e:#}"),
        )),
        (Ok(_), Ok(())) => Ok(()),
    }
}

fn cred_mtime(acc: &Account) -> Option<DateTime<Utc>> {
    let path = acc.config_dir().join(acc.harness.spec().cred_file);
    let t: SystemTime = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(t.into())
}

/// Should the dashboard fetch this account now?
///
/// Yes when the last attempt is older than the refresh interval. An account
/// whose last attempt needed the user (expired or rejected login) is asked
/// again only once its credentials file has changed, so a dead login is not
/// polled every ten minutes for ever.
pub fn due(acc: &Account, settings: &Settings, now: DateTime<Utc>) -> bool {
    askable(acc, now)
        && acc
            .meta
            .refresh
            .last_attempt
            .is_none_or(|last| now - last >= Duration::minutes(settings.refresh_minutes as i64))
}

/// May this account be fetched at all now, whatever its last fetch was? Not
/// without a login, not while a dead login waits for the user, and not while
/// a rate limit asks for quiet.
fn askable(acc: &Account, now: DateTime<Utc>) -> bool {
    if !acc.harness.logged_in(&acc.config_dir()) {
        return false;
    }
    let Some(last) = acc.meta.refresh.last_attempt else {
        return true;
    };
    if acc.meta.refresh.needs_user {
        return cred_mtime(acc).is_some_and(|m| m > last);
    }
    !acc.meta.refresh.retry_at.is_some_and(|t| t > now)
}

/// Half the refresh interval. An account fetched more recently than this is
/// left alone by a round, and by a second dashboard.
fn half(settings: &Settings) -> Duration {
    Duration::seconds(settings.refresh_minutes as i64 * 30)
}

/// The account whose answer a root login takes instead of asking itself: an
/// account that holds the same login, signed in and working. One login has
/// one usage, so asking twice only doubles the requests against it.
pub fn carrier<'a>(all: &'a [Account], root: &Account) -> Option<&'a Account> {
    if !root.is_native() {
        return None;
    }
    all.iter().find(|o| {
        !o.is_native()
            && o.harness == root.harness
            && same_usage(root, o)
            && !o.meta.refresh.needs_user
            && o.harness.logged_in(&o.config_dir())
    })
}

fn same_usage(a: &Account, b: &Account) -> bool {
    let orgs = match (&a.meta.identity.org, &b.meta.identity.org) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    };
    manage::same_login(a, b) && orgs
}

/// The accounts the dashboard fetches now. Nothing until one account is due.
/// Then that one and, with it, every account past half its wait, so accounts
/// last fetched at different moments fall into step: one round per interval
/// instead of a trickle of requests. `skip` leaves accounts out: the ones
/// being fetched already.
pub fn round(
    all: &[Account],
    settings: &Settings,
    now: DateTime<Utc>,
    skip: impl Fn(&Account) -> bool,
) -> Vec<String> {
    let mine = |a: &&Account| !skip(a) && carrier(all, a).is_none();
    if !all.iter().filter(mine).any(|a| due(a, settings, now)) {
        return Vec::new();
    }
    let half = half(settings);
    all.iter()
        .filter(mine)
        .filter(|a| askable(a, now))
        .filter(|a| a.meta.refresh.last_attempt.is_none_or(|t| now - t >= half))
        .map(Account::key)
        .collect()
}

/// Take the fetch of this account for this process. No when another process
/// fetched it, or is fetching it, within half the interval. Read and written
/// under the account's lock, so two dashboards cannot both say yes.
fn claim(acc: &mut Account, settings: &Settings, now: DateTime<Utc>) -> anyhow::Result<bool> {
    let half = half(settings);
    let mut mine = false;
    acc.update(|m| {
        if m.refresh.last_attempt.is_none_or(|t| now - t >= half) {
            m.refresh.last_attempt = Some(now);
            mine = true;
        }
    })?;
    Ok(mine)
}

/// Fetch the account `key` and store the outcome, also for the root login
/// that is the same login. With `scheduled`, the fetch is the dashboard's own
/// and is skipped when another dashboard has just made it. Says whether a
/// request was made.
pub fn fetch(key: &str, scheduled: Option<&Settings>) -> Result<bool, String> {
    let mut reg = Registry::load().map_err(|e| format!("{e:#}"))?;
    let acc = reg
        .get_mut(key)
        .ok_or_else(|| "the account is gone".to_string())?;
    if let Some(settings) = scheduled {
        if !claim(acc, settings, Utc::now()).map_err(|e| format!("{e:#}"))? {
            return Ok(false);
        }
    }
    let result = refresh_account(acc);
    let done = acc.clone();
    // A dead login says nothing about the root login's own.
    if !done.is_native() && !done.meta.refresh.needs_user {
        let riders = reg
            .accounts
            .iter_mut()
            .filter(|r| r.is_native() && r.harness == done.harness && same_usage(r, &done));
        for root in riders {
            let refresh = done.meta.refresh.clone();
            let _ = root.update(|m| m.refresh = refresh);
            if let (Ok(()), Some(snap)) = (&result, done.snapshot()) {
                history::record(&root.key(), snap);
            }
        }
    }
    result.map(|()| true).map_err(|e| e.text)
}

/// When the dashboard will next fetch this account, for the header countdown.
pub fn next_due(acc: &Account, settings: &Settings) -> Option<DateTime<Utc>> {
    if acc.meta.refresh.needs_user || !acc.harness.logged_in(&acc.config_dir()) {
        return None;
    }
    let last = acc.meta.refresh.last_attempt?;
    let next = last + Duration::minutes(settings.refresh_minutes as i64);
    Some(acc.meta.refresh.retry_at.map_or(next, |t| t.max(next)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;
    use crate::store::{AccountMeta, Billing, Identity};
    use crate::testutil::Sandbox;

    fn acc(id: u32) -> Account {
        Account {
            harness: Harness::Claude,
            meta: AccountMeta {
                harness: "claude".into(),
                id,
                ..AccountMeta::default()
            },
        }
    }

    #[test]
    fn a_different_login_drops_the_usage_of_the_one_before() {
        let who = |email: &str| Local {
            identity: Identity {
                email: Some(email.into()),
                ..Identity::default()
            },
            ..Local::default()
        };
        let mut meta = AccountMeta::default();
        merge(&mut meta, &who("a@example.com"));
        meta.refresh.data = Some(crate::testutil::week(10.0, 20.0, 48));
        // The same login again keeps it.
        merge(&mut meta, &who("a@example.com"));
        assert!(meta.refresh.data.is_some());
        merge(&mut meta, &who("b@example.com"));
        assert!(meta.refresh.data.is_none());
        assert_eq!(meta.identity.email.as_deref(), Some("b@example.com"));
    }

    #[test]
    fn hand_entered_plan_and_billing_survive_a_merge() {
        let mut meta = AccountMeta {
            plan: Some("My plan".into()),
            plan_source: Source::Manual,
            ..AccountMeta::default()
        };
        meta.billing = Billing {
            renews_on: chrono::NaiveDate::from_ymd_opt(2026, 12, 1),
            source: Source::Manual,
            ..Billing::default()
        };
        let local = Local {
            identity: Identity {
                email: Some("a@b.c".into()),
                ..Identity::default()
            },
            plan: Some("Max 20x".into()),
            billing: Some(Billing {
                renews_on: chrono::NaiveDate::from_ymd_opt(2027, 1, 1),
                source: Source::Api,
                ..Billing::default()
            }),
            ..Local::default()
        };
        merge(&mut meta, &local);
        assert_eq!(meta.identity.email.as_deref(), Some("a@b.c"));
        assert_eq!(meta.plan.as_deref(), Some("My plan"));
        assert_eq!(meta.billing.renews_on.unwrap().to_string(), "2026-12-01");
    }

    #[test]
    fn an_account_is_due_when_never_fetched_or_stale_but_not_when_logged_out() {
        let sb = Sandbox::new();
        let mut a = acc(1);
        let s = Settings::default();
        let now = Utc::now();
        assert!(!due(&a, &s, now), "no login, nothing to fetch");
        sb.login(&a);
        assert!(due(&a, &s, now));
        a.meta.refresh.last_attempt = Some(now - Duration::minutes(3));
        assert!(!due(&a, &s, now));
        a.meta.refresh.last_attempt = Some(now - Duration::minutes(11));
        assert!(due(&a, &s, now));
    }

    #[test]
    fn a_login_that_needs_the_user_waits_for_new_credentials() {
        let sb = Sandbox::new();
        let mut a = acc(1);
        sb.login(&a);
        let s = Settings::default();
        a.meta.refresh.needs_user = true;
        // Attempted after the credentials were written: stay quiet.
        a.meta.refresh.last_attempt = Some(Utc::now() + Duration::seconds(5));
        assert!(!due(&a, &s, Utc::now() + Duration::hours(5)));
        // Credentials newer than the attempt: ask again.
        a.meta.refresh.last_attempt = Some(Utc::now() - Duration::hours(1));
        assert!(due(&a, &s, Utc::now()));
    }

    fn fetched(id: u32, minutes_ago: i64, now: DateTime<Utc>) -> Account {
        let mut a = acc(id);
        a.meta.refresh.last_attempt = Some(now - Duration::minutes(minutes_ago));
        a
    }

    #[test]
    fn a_round_waits_for_one_due_account_then_takes_the_ones_past_half_their_wait() {
        let sb = Sandbox::new();
        let s = Settings::default();
        assert_eq!(s.refresh_minutes, 10);
        let now = Utc::now();
        // Fetched 9, 6 and 2 minutes ago: nobody is due, so nothing is asked.
        let mut all = vec![fetched(1, 9, now), fetched(2, 6, now), fetched(3, 2, now)];
        for a in &all {
            sb.login(a);
        }
        assert!(round(&all, &s, now, |_| false).is_empty());
        // The first comes due. The second is past half its wait and joins, so
        // the two are in step from now on. The third was asked too recently.
        all[0].meta.refresh.last_attempt = Some(now - Duration::minutes(10));
        assert_eq!(round(&all, &s, now, |_| false), ["claude-1", "claude-2"]);
        // One that is being fetched already is left out.
        assert_eq!(round(&all, &s, now, |a| a.meta.id == 1), [] as [&str; 0]);
        // A rate limit keeps an account out of the round it would have joined.
        all[1].meta.refresh.retry_at = Some(now + Duration::minutes(20));
        assert_eq!(round(&all, &s, now, |_| false), ["claude-1"]);
    }

    #[test]
    fn a_root_login_takes_the_answer_of_the_account_with_the_same_login() {
        let sb = Sandbox::new();
        let s = Settings::default();
        let now = Utc::now();
        let who = |email: &str| Identity {
            email: Some(email.into()),
            ..Identity::default()
        };
        let mut root = acc(0);
        root.meta.identity = who("me@example.com");
        let mut same = acc(1);
        same.meta.identity = who("me@example.com");
        let mut other = acc(2);
        other.meta.identity = who("you@example.com");
        for a in [&root, &same, &other] {
            sb.login(a);
        }
        let mut all = vec![root, same, other];
        // Never fetched: all are due, and the root login is asked through
        // the account that holds its login.
        assert_eq!(round(&all, &s, now, |_| false), ["claude-1", "claude-2"]);
        assert_eq!(
            carrier(&all, &all[0]).map(Account::key).as_deref(),
            Some("claude-1")
        );
        assert!(carrier(&all, &all[2]).is_none());
        // That account's login died: the root login asks for itself again.
        all[1].meta.refresh.needs_user = true;
        all[1].meta.refresh.last_attempt = Some(now + Duration::seconds(5));
        assert_eq!(round(&all, &s, now, |_| false), ["claude-0", "claude-2"]);
    }

    #[test]
    fn a_fetch_another_dashboard_just_made_is_not_made_again() {
        let sb = Sandbox::new();
        sb.demo();
        let s = Settings::default();
        let now = Utc::now();
        let mut reg = Registry::load().unwrap();
        let acc = reg.accounts.iter_mut().find(|a| !a.is_native()).unwrap();
        acc.update(|m| m.refresh.last_attempt = Some(now - Duration::minutes(30)))
            .unwrap();
        assert!(claim(acc, &s, now).unwrap(), "stale: this process takes it");
        assert!(!claim(acc, &s, now).unwrap(), "taken a moment ago");
        assert!(claim(acc, &s, now + Duration::minutes(6)).unwrap());
    }
}
