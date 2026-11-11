//! Usage samples over time, one JSON line per successful fetch. This is what
//! the analytics page draws from.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;

use crate::model::Snapshot;
use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub t: DateTime<Utc>,
    /// Weekly usage, 0–100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week: Option<f64>,
    /// Session-window usage, 0–100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<f64>,
    /// When the weekly window this sample belongs to resets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_reset: Option<DateTime<Utc>>,
}

/// Append one sample for `key`. Best effort: a history that cannot be written
/// must never fail a refresh.
pub fn record(key: &str, snap: &Snapshot) {
    let sample = Sample {
        t: snap.fetched_at,
        week: snap.weekly().map(|w| w.used_pct),
        session: snap.session().map(|w| w.used_pct),
        week_reset: snap.weekly().and_then(|w| w.resets_at),
    };
    let path = paths::history_file(key);
    let Some(parent) = path.parent() else { return };
    if crate::fsx::private_dir(parent).is_err() {
        return;
    }
    let Ok(line) = serde_json::to_string(&sample) else {
        return;
    };
    let mut opts = OpenOptions::new();
    opts.append(true).create(true);
    if let Ok(mut f) = crate::platform::private_open(&mut opts).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

/// Every sample for `key`, oldest first. Lines that do not parse are skipped.
pub fn load(key: &str) -> Vec<Sample> {
    let Ok(text) = fs::read_to_string(paths::history_file(key)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Drop samples older than `days`. Called once when the dashboard opens.
pub fn prune(key: &str, days: u32) {
    let path = paths::history_file(key);
    let samples = load(key);
    if samples.is_empty() {
        return;
    }
    let cutoff = Utc::now() - Duration::days(days as i64);
    let kept: Vec<&Sample> = samples.iter().filter(|s| s.t >= cutoff).collect();
    if kept.len() == samples.len() {
        return;
    }
    let text: String = kept
        .iter()
        .filter_map(|s| serde_json::to_string(s).ok())
        .map(|l| l + "\n")
        .collect();
    let _ = crate::fsx::write_private(&path, text.as_bytes());
}

/// Weekly usage gained per day over the last `hours`, from the samples of the
/// current weekly window only (a reset in between would read as negative).
pub fn burn_per_day(samples: &[Sample], hours: i64, now: DateTime<Utc>) -> Option<f64> {
    let last = samples.last()?;
    let reset = last.week_reset?;
    let since = now - Duration::hours(hours);
    let window: Vec<&Sample> = samples
        .iter()
        .filter(|s| s.week_reset == Some(reset) && s.t >= since && s.week.is_some())
        .collect();
    let (first, last) = (window.first()?, window.last()?);
    let span = (last.t - first.t).num_minutes() as f64 / 1440.0;
    if span < 1.0 / 24.0 {
        return None;
    }
    Some(((last.week? - first.week?) / span).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(hours_ago: i64, week: f64, reset: DateTime<Utc>) -> Sample {
        Sample {
            t: Utc::now() - Duration::hours(hours_ago),
            week: Some(week),
            session: None,
            week_reset: Some(reset),
        }
    }

    #[test]
    fn burn_rate_uses_only_the_current_week() {
        let now = Utc::now();
        let (old, cur) = (now - Duration::days(1), now + Duration::days(3));
        let samples = vec![s(60, 90.0, old), s(24, 10.0, cur), s(0, 30.0, cur)];
        let rate = burn_per_day(&samples, 72, now).unwrap();
        assert!((rate - 20.0).abs() < 0.5, "{rate}");
    }

    #[test]
    fn too_little_history_gives_no_rate() {
        let now = Utc::now();
        let cur = now + Duration::days(3);
        assert!(burn_per_day(&[s(0, 30.0, cur)], 72, now).is_none());
        assert!(burn_per_day(&[], 72, now).is_none());
    }
}
