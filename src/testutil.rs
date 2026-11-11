//! A fabricated `$HOME` for unit tests. Environment variables are shared by
//! the whole test process, so a sandbox holds a lock for as long as it lives:
//! tests that use one run one at a time.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::harness::Harness;
use crate::store::Account;

static LOCK: Mutex<()> = Mutex::new(());

pub struct Sandbox {
    dir: tempfile::TempDir,
    _guard: MutexGuard<'static, ()>,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();
        std::env::set_var("HOME", &home);
        std::env::set_var("CSWAP_HOME", home.join(".cswap"));
        // The 0.6 layout is found through these; a test must not see the
        // real ones.
        std::env::set_var("CSWAP_PROC", dir.path().join("proc"));
        std::env::set_var("CSWAP_OFFLINE", "1");
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("XDG_DATA_HOME");
        for h in Harness::ALL {
            std::env::remove_var(format!("CSWAP_NATIVE_{}", h.id().to_uppercase()));
            std::env::remove_var(format!("CSWAP_{}", h.id().to_uppercase()));
            std::env::remove_var(format!("CSWAP_{}_BIN", h.id().to_uppercase()));
        }
        Sandbox { dir, _guard: guard }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Create and return the native config dir of `h`.
    pub fn native(&self, h: Harness) -> PathBuf {
        let p = h.native_dir();
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// Give `acc` a login that looks valid for years, so nothing tries the
    /// network.
    pub fn login(&self, acc: &Account) {
        let dir = acc.config_dir();
        let (file, body) = match acc.harness {
            Harness::Claude => (
                ".credentials.json",
                r#"{"claudeAiOauth":{"accessToken":"tok","refreshToken":"r","expiresAt":4102444800000}}"#,
            ),
            Harness::Codex => (
                "auth.json",
                r#"{"tokens":{"access_token":"a.b.c","account_id":"acct"}}"#,
            ),
            Harness::Grok => (
                "auth.json",
                r#"{"https://auth.x.ai::c":{"key":"k","email":"g@x.ai"}}"#,
            ),
            Harness::Agy => (
                "antigravity-cli/antigravity-oauth-token",
                r#"{"token":{"refresh_token":"r"},"id_token_email":"a@g.co"}"#,
            ),
        };
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
}

// ------------------------------------------------------------------ a demo

use chrono::{Duration, Utc};

use crate::model::{Credits, Grant, Snapshot, Source, Window, WindowKind};
use crate::store::{AccountMeta, Billing, Cycle};

fn window(kind: WindowKind, label: &str, used: f64, reset_h: i64, secs: i64) -> Window {
    Window {
        kind,
        label: label.to_string(),
        used_pct: used,
        resets_at: Some(Utc::now() + Duration::hours(reset_h)),
        window_secs: Some(secs),
    }
}

/// A snapshot with a 5-hour and a weekly window, as Claude and Codex report.
pub fn week(session: f64, weekly: f64, reset_h: i64) -> Snapshot {
    Snapshot::new(vec![
        window(WindowKind::Session, "5h", session, 2, 18_000),
        window(WindowKind::Weekly, "week", weekly, reset_h, 604_800),
    ])
}

/// A fake agent: it records how it was started, and its sign-in writes a
/// login into whatever config dir it was pointed at.
const FAKE_AGENT: &str = r#"#!/bin/sh
name=$(basename "$0")
log="${CSWAP_FAKE_LOG:-/dev/null}"
{
  echo "argv: $name $*"
  echo "CLAUDE_CONFIG_DIR=${CLAUDE_CONFIG_DIR:-}"
  echo "CODEX_HOME=${CODEX_HOME:-}"
  echo "GROK_HOME=${GROK_HOME:-}"
  echo "CSWAP_ACCOUNT=${CSWAP_ACCOUNT:-}"
} >> "$log"
case "$name $*" in
  "claude auth login")
    printf '%s' '{"claudeAiOauth":{"accessToken":"tok","refreshToken":"r","expiresAt":4102444800000}}' > "$CLAUDE_CONFIG_DIR/.credentials.json"
    printf '%s' '{"oauthAccount":{"emailAddress":"new@example.com"}}' > "$CLAUDE_CONFIG_DIR/.claude.json"
    echo "Login successful." ;;
  "codex login")
    if [ -n "${CSWAP_FAKE_FAIL:-}" ]; then echo "login cancelled"; exit 1; fi
    printf '%s' '{"tokens":{"access_token":"a.b.c","account_id":"acct-new"}}' > "$CODEX_HOME/auth.json"
    echo "Logged in." ;;
  *) echo "$name ran: $*" ;;
esac
"#;

impl Sandbox {
    /// Put fake `claude`, `codex`, `agy` and `grok` programs in place and tell
    /// cswap to use them. Returns the file they log to.
    pub fn fake_agents(&self) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = self.root().join("bin");
        fs::create_dir_all(&bin).unwrap();
        for h in Harness::ALL {
            let path = bin.join(h.spec().bins[0]);
            fs::write(&path, FAKE_AGENT).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            std::env::set_var(format!("CSWAP_{}_BIN", h.id().to_uppercase()), &path);
        }
        let log = self.root().join("agents.log");
        std::env::set_var("CSWAP_FAKE_LOG", &log);
        log
    }

    fn store(&self, h: Harness, id: u32, f: impl FnOnce(&mut AccountMeta)) -> Account {
        let mut acc = Account {
            harness: h,
            meta: AccountMeta {
                harness: h.id().to_string(),
                id,
                ..AccountMeta::default()
            },
        };
        crate::fsx::private_dir(&acc.dir()).unwrap();
        self.login(&acc);
        let now = Utc::now();
        acc.update(|m| {
            // Looked at a moment ago, so nothing in a test goes to the network.
            m.refresh.last_attempt = Some(now);
            m.refresh.last_success = Some(now - Duration::minutes(3));
            f(m)
        })
        .unwrap();
        acc
    }

    /// A home with every agent, several accounts and enough history to fill
    /// each page of the dashboard.
    pub fn demo(&self) {
        let now = Utc::now();
        self.fake_agents();
        crate::store::Settings::update(|s| s.refresh_on_open = false).unwrap();
        for h in Harness::ALL {
            let native = self.native(h);
            for name in ["settings.json", "history.jsonl"] {
                fs::write(native.join(name), "{}").unwrap();
            }
            for dir in ["projects", "skills", "plugins"] {
                fs::create_dir_all(native.join(dir)).unwrap();
            }
        }
        // Claude Code keeps who is signed in beside its config dir.
        fs::write(
            crate::paths::home().join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"dev@example.com","organizationName":"Personal"}}"#,
        )
        .unwrap();

        // Claude: the default login nearly spent, a work account with room and
        // a spare reset, and one that is cancelled.
        self.store(Harness::Claude, 0, |m| {
            m.plan = Some("Max".into());
            m.refresh.data = Some(week(64.0, 91.0, 30));
        });
        let work = crate::manage::create(Harness::Claude, "work").unwrap();
        self.store(Harness::Claude, work.meta.id, |m| {
            m.identity.email = Some("dev@corp.example".into());
            m.identity.org = Some("Corp".into());
            m.plan = Some("Ultra".into());
            m.last_used = Some(now - Duration::hours(2));
            m.billing = Billing {
                cycle: Some(Cycle::Monthly),
                renews_on: Some(crate::store::today() + Duration::days(12)),
                cancelled: false,
                source: Source::Manual,
            };
            let mut snap = week(12.0, 38.0, 9);
            snap.windows
                .push(window(WindowKind::WeeklyScoped, "Opus", 71.0, 9, 604_800));
            snap.credits = Some(Credits {
                enabled: true,
                used: Some(12.4),
                limit: Some(50.0),
                balance: None,
                unit: "USD".into(),
                resets_at: None,
            });
            snap.grants = vec![Grant {
                scope: "weekly".into(),
                count: 1,
                expires_at: Some(now + Duration::hours(52)),
                note: String::new(),
                source: Source::Api,
                usable_now: Some(false),
            }];
            m.refresh.data = Some(snap);
        });
        let side = crate::manage::create(Harness::Claude, "side").unwrap();
        self.store(Harness::Claude, side.meta.id, |m| {
            m.identity.email = Some("side@example.com".into());
            m.plan = Some("Pro".into());
            m.billing = Billing {
                cycle: Some(Cycle::Monthly),
                renews_on: Some(crate::store::today() + Duration::days(4)),
                cancelled: true,
                source: Source::Manual,
            };
            m.refresh.data = Some(week(0.0, 100.0, 70));
        });

        // Codex: the default login, and a second account that never signed in.
        self.store(Harness::Codex, 0, |m| {
            m.identity.email = Some("dev@example.com".into());
            m.plan = Some("Plus".into());
            m.refresh.data = Some(week(3.0, 22.0, 120));
        });
        crate::manage::create(Harness::Codex, "team").unwrap();

        // Grok reports one monthly credit window.
        self.store(Harness::Grok, 0, |m| {
            m.plan = Some("SuperGrok".into());
            m.refresh.data = Some(Snapshot::new(vec![window(
                WindowKind::Monthly,
                "month",
                47.0,
                400,
                2_592_000,
            )]));
        });
        // Antigravity is signed in but its last fetch failed.
        self.store(Harness::Agy, 0, |m| {
            m.refresh.error = Some("agy did not answer within 30 s".into());
        });

        // A week of samples for the work account, for the analytics page.
        let reset = now + Duration::hours(9);
        for (hours_ago, pct) in [
            (150, 2.0),
            (120, 9.0),
            (96, 14.0),
            (72, 21.0),
            (48, 27.0),
            (24, 33.0),
            (2, 38.0),
        ] {
            let mut snap = week(0.0, pct, 9);
            snap.fetched_at = now - Duration::hours(hours_ago);
            snap.windows[1].resets_at = Some(reset);
            crate::history::record(&work.key(), &snap);
        }
    }
}

impl Sandbox {
    /// Keep the fabricated home on disk after the test and return its root.
    pub fn keep(self) -> PathBuf {
        self.dir.keep()
    }
}

/// `cargo test demo_home -- --ignored --nocapture` writes the demo home to
/// disk and prints how to run cswap against it by hand.
#[test]
#[ignore = "writes a demo home to look at by hand"]
fn demo_home() {
    let sb = Sandbox::new();
    sb.demo();
    let root = sb.keep();
    println!("DEMO_ROOT={}", root.display());
}
