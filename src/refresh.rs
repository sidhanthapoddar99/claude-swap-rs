//! Fetch one account's usage and store the outcome in its settings.
//!
//! Only the open dashboard and the explicit `cswap refresh` call this. A
//! wrapped launch never does: it reads what was last stored.

use chrono::{DateTime, Duration, Utc};
use std::time::SystemTime;

use crate::harness::Local;
use crate::history;
use crate::http::FetchError;
use crate::model::Source;
use crate::store::{Account, AccountMeta, Settings};

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
    if !acc.harness.logged_in(&acc.config_dir()) {
        return false;
    }
    let Some(last) = acc.meta.refresh.last_attempt else {
        return true;
    };
    if acc.meta.refresh.needs_user {
        return cred_mtime(acc).is_some_and(|m| m > last);
    }
    if acc.meta.refresh.retry_at.is_some_and(|t| t > now) {
        return false;
    }
    now - last >= Duration::minutes(settings.refresh_minutes as i64)
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
}
