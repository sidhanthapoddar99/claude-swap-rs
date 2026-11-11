//! What every harness reports, in one shape: usage windows, extra credits and
//! reset grants. Each harness module turns its own API response into this.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// The kind of limit a window measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    /// The short rolling window (5 hours on Claude and Codex).
    Session,
    /// The account-wide weekly limit.
    Weekly,
    /// A weekly limit for one model or one feature.
    WeeklyScoped,
    Daily,
    Monthly,
    Other,
}

impl WindowKind {
    /// Classify by window length when the API gives only seconds.
    pub fn from_secs(secs: i64) -> WindowKind {
        match secs {
            s if s <= 0 => WindowKind::Other,
            s if s <= 12 * 3600 => WindowKind::Session,
            s if s <= 36 * 3600 => WindowKind::Daily,
            s if s <= 8 * 86400 => WindowKind::Weekly,
            s if s <= 32 * 86400 => WindowKind::Monthly,
            _ => WindowKind::Other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub kind: WindowKind,
    /// Short label, such as `5h`, `week`, `Opus`.
    pub label: String,
    /// 0–100, as of `Snapshot::fetched_at`.
    pub used_pct: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
    /// Window length, when known. Lets the UI show pace against time elapsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_secs: Option<i64>,
}

impl Window {
    /// The stretch of time this limit covers, as a word for a sentence:
    /// `week`. The label cannot stand in for it, because it may name a group
    /// of models instead (`Gemini`).
    pub fn period(&self) -> &str {
        match self.kind {
            WindowKind::Weekly => "week",
            WindowKind::Monthly => "month",
            WindowKind::Daily => "day",
            _ => &self.label,
        }
    }

    /// Usage right now. A window whose reset time has passed since the fetch
    /// has started again from zero, whatever the stored number says.
    pub fn used_now(&self, now: DateTime<Utc>) -> f64 {
        match self.resets_at {
            Some(t) if t <= now => 0.0,
            _ => self.used_pct.clamp(0.0, 100.0),
        }
    }

    /// Has the stored reset time already passed?
    pub fn rolled_over(&self, now: DateTime<Utc>) -> bool {
        self.resets_at.is_some_and(|t| t <= now)
    }

    /// Share of the window's time that has elapsed, 0–100. `None` when the
    /// length or the reset time is unknown, or the window has rolled over.
    pub fn elapsed_pct(&self, now: DateTime<Utc>) -> Option<f64> {
        let (reset, secs) = (self.resets_at?, self.window_secs?);
        if secs <= 0 || reset <= now {
            return None;
        }
        let left = (reset - now).num_seconds() as f64;
        Some(((1.0 - left / secs as f64) * 100.0).clamp(0.0, 100.0))
    }

    /// Projected usage at reset if the current pace holds. `None` when there
    /// is too little of the window behind us to say.
    pub fn projected_pct(&self, now: DateTime<Utc>) -> Option<f64> {
        let elapsed = self.elapsed_pct(now)?;
        if elapsed < 5.0 {
            return None;
        }
        Some(self.used_now(now) / elapsed * 100.0)
    }
}

/// Pay-as-you-go usage on top of the plan (Claude "extra usage", Codex
/// credits).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Credits {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<f64>,
    /// `USD`, `credits`, …
    #[serde(default)]
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
}

/// Where a fact came from. Manual entries are never overwritten by a fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    #[default]
    Manual,
    Api,
}

/// A reset the account can still spend: it puts a limit back to zero before
/// its normal reset time, and is lost if not used by `expires_at`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    /// What it resets: `weekly`, `session`, `monthly`, …
    pub scope: String,
    #[serde(default = "one")]
    pub count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default)]
    pub source: Source,
    /// Whether the vendor says it can be spent right now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usable_now: Option<bool>,
}

fn one() -> u32 {
    1
}

impl Grant {
    pub fn expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|t| t <= now)
    }
}

/// One successful usage fetch for one account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub fetched_at: DateTime<Utc>,
    #[serde(default)]
    pub windows: Vec<Window>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<Credits>,
    /// Resets the API itself reports as available.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
    /// When `grants` was last read. They are asked for less often than usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grants_at: Option<DateTime<Utc>>,
    /// What the usage answer revealed about how the plan bills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewal_hint: Option<crate::store::Billing>,
}

impl Snapshot {
    pub fn new(windows: Vec<Window>) -> Snapshot {
        Snapshot {
            fetched_at: Utc::now(),
            windows,
            credits: None,
            grants: Vec::new(),
            grants_at: None,
            renewal_hint: None,
        }
    }

    pub fn first(&self, kind: WindowKind) -> Option<&Window> {
        self.windows.iter().find(|w| w.kind == kind)
    }

    /// The account-wide weekly window; the longest account-wide window when the
    /// harness has no weekly one (so every account has a "main" bar).
    pub fn weekly(&self) -> Option<&Window> {
        self.first(WindowKind::Weekly)
            .or_else(|| self.first(WindowKind::Monthly))
            .or_else(|| self.first(WindowKind::Daily))
    }

    pub fn session(&self) -> Option<&Window> {
        self.first(WindowKind::Session)
    }

    /// The window that blocks the account right now, if any: an account-wide
    /// limit at 100% whose reset is still ahead.
    pub fn blocking(&self, now: DateTime<Utc>) -> Option<&Window> {
        self.windows
            .iter()
            .filter(|w| w.kind != WindowKind::WeeklyScoped && w.kind != WindowKind::Other)
            .filter(|w| w.used_now(now) >= 99.5)
            .max_by_key(|w| w.resets_at)
    }
}

// ---------------------------------------------------------------- time text

/// `5d 3h`, `2h 47m`, `47m`, `now`: two units at most, coarsest first.
pub fn span(d: Duration) -> String {
    let mins = d.num_minutes();
    if mins <= 0 {
        return "now".to_string();
    }
    let (days, hours, m) = (mins / 1440, (mins % 1440) / 60, mins % 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

/// `just now`, `4m ago`, `3h ago`, `2d ago`.
pub fn ago(t: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - t).num_seconds();
    match secs {
        s if s < 45 => "just now".to_string(),
        s if s < 3600 => format!("{}m ago", (s + 30) / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86400),
    }
}

/// `Sat 14 Nov · 09:30` in local time; `today · 09:30` and `tomorrow · 09:30`
/// for the two nearest days.
pub fn landing(t: DateTime<Utc>, now: DateTime<Utc>, clock_24h: bool) -> String {
    let local = t.with_timezone(&chrono::Local);
    let today = now.with_timezone(&chrono::Local).date_naive();
    let clock = if clock_24h {
        local.format("%H:%M").to_string()
    } else {
        local.format("%-I:%M %p").to_string().to_lowercase()
    };
    let days = (local.date_naive() - today).num_days();
    let day = match days {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        _ => local.format("%a %-d %b").to_string(),
    };
    format!("{day} · {clock}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(pct: f64, reset_in_h: i64, secs: i64) -> Window {
        Window {
            kind: WindowKind::Weekly,
            label: "week".into(),
            used_pct: pct,
            resets_at: Some(Utc::now() + Duration::hours(reset_in_h)),
            window_secs: Some(secs),
        }
    }

    #[test]
    fn a_window_past_its_reset_reads_zero() {
        let w = win(88.0, -1, 7 * 86400);
        assert_eq!(w.used_now(Utc::now()), 0.0);
        assert!(w.rolled_over(Utc::now()));
        assert!(w.elapsed_pct(Utc::now()).is_none());
    }

    #[test]
    fn pace_projects_from_time_elapsed() {
        // Half the week gone, 25% used: on pace for 50%.
        let w = win(25.0, 84, 7 * 86400);
        let now = Utc::now();
        assert!((w.elapsed_pct(now).unwrap() - 50.0).abs() < 0.1);
        assert!((w.projected_pct(now).unwrap() - 50.0).abs() < 0.5);
    }

    #[test]
    fn kinds_from_window_length() {
        assert_eq!(WindowKind::from_secs(18000), WindowKind::Session);
        assert_eq!(WindowKind::from_secs(86400), WindowKind::Daily);
        assert_eq!(WindowKind::from_secs(604800), WindowKind::Weekly);
        assert_eq!(WindowKind::from_secs(30 * 86400), WindowKind::Monthly);
    }

    #[test]
    fn spans_use_two_units() {
        assert_eq!(span(Duration::minutes(0)), "now");
        assert_eq!(span(Duration::minutes(47)), "47m");
        assert_eq!(span(Duration::minutes(167)), "2h 47m");
        assert_eq!(span(Duration::minutes(7380)), "5d 3h");
    }

    #[test]
    fn blocking_ignores_scoped_and_rolled_over_windows() {
        let now = Utc::now();
        let mut scoped = win(100.0, 5, 604800);
        scoped.kind = WindowKind::WeeklyScoped;
        let snap = Snapshot::new(vec![scoped, win(100.0, -2, 604800)]);
        assert!(snap.blocking(now).is_none());
        let snap = Snapshot::new(vec![win(100.0, 3, 604800)]);
        assert!(snap.blocking(now).is_some());
    }
}
