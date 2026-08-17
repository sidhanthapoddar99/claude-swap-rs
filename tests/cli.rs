//! End-to-end tests against the real binary, inside a fabricated $HOME.
//!
//! Nothing here touches the developer's actual ~/.claude: every invocation
//! gets HOME/XDG_* pointed at a TempDir, and credentials carry a far-future
//! expiresAt so no code path ever reaches the network. All runs are
//! non-interactive (no tty), so interactive pickers are never entered —
//! commands take the argument/fallback paths.
//!
//! The model under test: one profile per account, keyed by email; `default` is
//! the live ~/.claude, which cswap only reads and which is the one thing
//! allowed to hold the same account as a profile.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_cswap");

/// Who is logged into the fabricated live ~/.claude.
const LIVE_EMAIL: &str = "live@x.com";
const LIVE_TOKEN: &str = "tok-live";

struct Env {
    home: TempDir,
}

impl Env {
    /// Fake $HOME with a live-looking ~/.claude + ~/.claude.json.
    fn new() -> Env {
        let home = TempDir::new().unwrap();
        let h = home.path();
        let claude = h.join(".claude");
        fs::create_dir_all(claude.join("projects/proj-a")).unwrap();
        fs::create_dir_all(claude.join("plugins/repos")).unwrap();
        fs::create_dir_all(claude.join("agents")).unwrap();
        fs::write(claude.join("settings.json"), r#"{"model":"opus"}"#).unwrap();
        fs::write(claude.join("CLAUDE.md"), "# rules\n").unwrap();
        fs::write(claude.join("history.jsonl"), "{\"display\":\"hi\"}\n").unwrap();
        fs::write(claude.join("projects/proj-a/s1.jsonl"), "{}\n").unwrap();
        // Denylisted names: a version-controlled ~/.claude and its
        // .claude.json backup snapshots. Neither may ever be linked.
        fs::create_dir_all(claude.join(".git/refs")).unwrap();
        fs::create_dir_all(claude.join("backups")).unwrap();
        fs::write(claude.join("backups/.claude.json.backup.1"), "{}\n").unwrap();
        fs::write(
            claude.join(".credentials.json"),
            Self::creds(LIVE_TOKEN).to_string(),
        )
        .unwrap();
        fs::write(
            h.join(".claude.json"),
            json!({
                "oauthAccount": {"emailAddress": LIVE_EMAIL, "accountUuid": "u-live"},
                "theme": "dark",
                "mcpServers": {"srv": {"command": "x"}},
                "projects": {"/tmp/repo": {"hasTrustDialogAccepted": true}},
            })
            .to_string(),
        )
        .unwrap();
        Env { home }
    }

    fn creds(token: &str) -> Value {
        json!({"claudeAiOauth": {
            "accessToken": token,
            "refreshToken": format!("r-{token}"),
            // Far future: refresh_if_needed never fires, so never any network.
            "expiresAt": 9_999_999_999_999i64,
            "scopes": ["user:inference"],
        }})
    }

    fn cswap(&self, args: &[&str]) -> Output {
        self.cswap_env(args, &[])
    }

    fn cswap_env(&self, args: &[&str], extra: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var("PATH").unwrap());
        for (k, v) in extra {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn data(&self) -> PathBuf {
        self.home.path().join(".local/share/cswap")
    }

    fn config_path(&self) -> PathBuf {
        self.home.path().join(".config/cswap/config.toml")
    }

    fn config_text(&self) -> String {
        fs::read_to_string(self.config_path()).unwrap_or_default()
    }

    fn profile(&self, email: &str) -> PathBuf {
        self.data().join("profiles").join(email)
    }

    fn live(&self) -> PathBuf {
        self.home.path().join(".claude")
    }

    fn token_of(&self, path: PathBuf) -> String {
        let v: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        v["claudeAiOauth"]["accessToken"].as_str().unwrap().into()
    }

    fn live_token(&self) -> String {
        self.token_of(self.live().join(".credentials.json"))
    }

    fn profile_token(&self, email: &str) -> String {
        self.token_of(self.profile(email).join(".credentials.json"))
    }

    /// A fake `claude` that performs a login inside $CLAUDE_CONFIG_DIR: writes
    /// credentials, and adds `oauthAccount` to whatever .claude.json cswap
    /// seeded (preserving the seeded keys, like the real claude does).
    fn fake_login(&self, email: &str, token: &str) -> PathBuf {
        self.write_script(
            &format!("fake-login-{token}.sh"),
            &format!(
                r#"#!/bin/sh
set -e
CFG="$CLAUDE_CONFIG_DIR"
cat > "$CFG/.credentials.json" <<'CREDS'
{creds}
CREDS
if [ -s "$CFG/.claude.json" ]; then
  head -n -1 "$CFG/.claude.json" > "$CFG/.claude.json.tmp"
  printf ',\n  "oauthAccount": {{"emailAddress": "{email}", "accountUuid": "u-{token}"}}\n}}\n' \
    >> "$CFG/.claude.json.tmp"
  mv "$CFG/.claude.json.tmp" "$CFG/.claude.json"
else
  printf '{{"oauthAccount": {{"emailAddress": "{email}"}}}}\n' > "$CFG/.claude.json"
fi
"#,
                creds = Self::creds(token),
            ),
        )
    }

    /// A login that also writes into the shared directories, the way a real
    /// claude session does. Used to prove the share links exist BEFORE claude
    /// runs, so its writes land in ~/.claude instead of forking the profile.
    fn fake_login_that_writes_shared(&self, email: &str, token: &str) -> PathBuf {
        let base = fs::read_to_string(self.fake_login(email, token)).unwrap();
        self.write_script(
            "fake-login-shared.sh",
            &format!(
                "{base}\nmkdir -p \"$CFG/plugins/newmarket\"\n\
                 echo probe > \"$CFG/projects/probe.txt\"\n"
            ),
        )
    }

    /// A fake `claude` that prints its CLAUDE_CONFIG_DIR and args, then exits.
    fn fake_claude(&self) -> PathBuf {
        self.write_script(
            "fake-claude.sh",
            "#!/bin/sh\necho \"CFG=$CLAUDE_CONFIG_DIR\"\necho \"ARGS=$*\"\n\
             echo \"KEY=${ANTHROPIC_API_KEY:-scrubbed}\"\n",
        )
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.home.path().join(name);
        fs::write(&path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// `cswap login <key>` driven by a fake claude that logs in as `arrives_as`.
    fn login_as(&self, key: &str, arrives_as: &str, token: &str) -> Output {
        let fake = self.fake_login(arrives_as, token);
        self.cswap_env(
            &["login", key, "--yes"],
            &[("CSWAP_CLAUDE_BIN", fake.to_str().unwrap())],
        )
    }

    /// The ordinary case: log in as the account you asked for.
    fn login(&self, email: &str, token: &str) -> Output {
        self.login_as(email, email, token)
    }

    /// `cswap run` with a fake claude that just reports its config dir.
    fn run_cmd(&self, args: &[&str], extra: &[(&str, &str)]) -> Output {
        let fake = self.fake_claude();
        let mut all = vec![("CSWAP_CLAUDE_BIN", fake.to_str().unwrap())];
        all.extend_from_slice(extra);
        self.cswap_env(args, &all)
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}
fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}
fn assert_ok(o: &Output) {
    assert!(
        o.status.success(),
        "stdout: {}\nstderr: {}",
        stdout(o),
        stderr(o)
    );
}

fn is_link_to(link: &Path, target: &Path) -> bool {
    fs::symlink_metadata(link)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
        && fs::read_link(link).map(|t| t == target).unwrap_or(false)
}

fn is_symlink(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

// ---------------------------------------------------------------------- login

#[test]
fn login_creates_a_profile_keyed_by_account_with_its_own_identity_files() {
    let env = Env::new();
    let o = env.login("work@x.com", "tok-work");
    assert_ok(&o);
    assert!(
        stdout(&o).contains("Created profile for work@x.com"),
        "{}",
        stdout(&o)
    );

    // Config is email-keyed, with no separate name key.
    let cfg = env.config_text();
    assert!(cfg.contains("[[profile]]"), "{cfg}");
    assert!(cfg.contains("email = \"work@x.com\""), "{cfg}");
    assert!(!cfg.contains("name ="), "{cfg}");
    assert!(!cfg.contains("[[account]]"), "{cfg}");

    // The directory is named for the account.
    let profile = env.profile("work@x.com");
    assert!(profile.is_dir(), "profiles/work@x.com must exist");

    // Identity: real files, private perms — claude's umask does not decide.
    use std::os::unix::fs::PermissionsExt;
    for f in [".credentials.json", ".claude.json"] {
        let p = profile.join(f);
        let md = fs::symlink_metadata(&p).unwrap();
        assert!(md.file_type().is_file(), "{f} must be a real file");
        assert_eq!(md.permissions().mode() & 0o777, 0o600, "{f} must be 0600");
    }

    // Seeded .claude.json survived the login: onboarding skip + carried keys.
    let cj: Value =
        serde_json::from_str(&fs::read_to_string(profile.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(cj["hasCompletedOnboarding"], json!(true));
    assert_eq!(cj["theme"], json!("dark"));
    assert!(cj["mcpServers"]["srv"].is_object(), "mcp carried over");
    assert_eq!(
        cj["projects"]["/tmp/repo"]["hasTrustDialogAccepted"],
        json!(true)
    );
    // The login itself recorded the identity — cswap never wrote one in.
    assert_eq!(cj["oauthAccount"]["emailAddress"], json!("work@x.com"));

    // Everything else: symlinks into ~/.claude (share-all-except-denylist).
    let src = env.live();
    for item in [
        "settings.json",
        "CLAUDE.md",
        "projects",
        "history.jsonl",
        "plugins",
        "agents",
    ] {
        assert!(
            is_link_to(&profile.join(item), &src.join(item)),
            "{item} should be a symlink into ~/.claude"
        );
    }
    // Denylist: never linked, however tempting the scan makes it look.
    for item in [".credentials.json", "backups", ".git"] {
        assert!(
            !is_symlink(&profile.join(item)),
            "{item} is denylisted and must never be a symlink into ~/.claude"
        );
    }
}

/// The bug this design was built around: claude used to be launched in a bare
/// staging directory, so it created real `plugins/`, `projects/` and
/// `sessions/` that then shadowed the share links forever.
#[test]
fn login_links_the_directory_before_claude_writes_to_it() {
    let env = Env::new();
    let fake = env.fake_login_that_writes_shared("work@x.com", "tok-work");
    assert_ok(&env.cswap_env(
        &["login", "work@x.com"],
        &[("CSWAP_CLAUDE_BIN", fake.to_str().unwrap())],
    ));

    let profile = env.profile("work@x.com");
    for item in ["plugins", "projects"] {
        assert!(
            is_link_to(&profile.join(item), &env.live().join(item)),
            "{item} must still be a share link after the login session"
        );
    }
    // And the login's writes landed in the shared directory, not a fork.
    assert!(
        env.live().join("plugins/newmarket").is_dir(),
        "a plugin installed during login belongs to ~/.claude"
    );
    assert!(
        env.live().join("projects/probe.txt").is_file(),
        "the login session's transcript belongs to ~/.claude"
    );
    assert!(
        !env.data().join("staging-login").exists(),
        "there is no staging directory in this model"
    );
}

#[test]
fn login_never_reads_writes_or_copies_the_live_login() {
    let env = Env::new();
    let live_before = fs::read_to_string(env.home.path().join(".claude.json")).unwrap();
    assert_ok(&env.login("work@x.com", "tok-work"));

    assert_eq!(env.live_token(), LIVE_TOKEN);
    assert_eq!(
        fs::read_to_string(env.home.path().join(".claude.json")).unwrap(),
        live_before,
        "cswap must not write ~/.claude.json"
    );
    assert_eq!(env.profile_token("work@x.com"), "tok-work");
    assert!(
        !env.data().join("accounts").exists(),
        "a second credential copy is exactly what this model removes"
    );
}

/// One profile per account. The only thing allowed to double up is `default`.
#[test]
fn a_second_profile_for_the_same_account_is_refused() {
    let env = Env::new();
    assert_ok(&env.login("same@x.com", "tok-a"));
    assert_ok(&env.cswap(&["alias", "create", "same@x.com", "work"]));

    // Naming the same account again is a RE-LOGIN of the existing profile,
    // never a second profile.
    let o = env.login("same@x.com", "tok-b");
    assert_ok(&o);
    assert!(
        stdout(&o).contains("Logged same@x.com in again"),
        "{}",
        stdout(&o)
    );
    assert_eq!(env.config_text().matches("[[profile]]").count(), 1);
    assert_eq!(env.profile_token("same@x.com"), "tok-b");
    assert!(env.config_text().contains("work"), "alias survived");

    // And logging a DIFFERENT profile into an account that is already taken is
    // caught rather than filed under the wrong directory.
    let o = env.login_as("other@x.com", "same@x.com", "tok-c");
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("you logged in as same@x.com"),
        "{}",
        stderr(&o)
    );
    assert!(
        !env.profile("other@x.com").exists(),
        "the half-built profile is cleaned up"
    );
    assert_eq!(env.profile_token("same@x.com"), "tok-b", "untouched");
}

/// `default` is the exception: it may hold an account a profile also holds.
#[test]
fn a_profile_may_hold_the_same_account_as_the_default() {
    let env = Env::new();
    let o = env.login(LIVE_EMAIL, "tok-mine");
    assert_ok(&o);
    assert!(
        stdout(&o).contains("also the default"),
        "the overlap is stated, not prevented: {}",
        stdout(&o)
    );

    // It runs from its own directory with its own tokens — the old model
    // silently fell back to passthrough here.
    let o = env.run_cmd(&["run", LIVE_EMAIL], &[]);
    assert_ok(&o);
    assert!(
        stdout(&o).contains(&format!("CFG={}", env.profile(LIVE_EMAIL).display())),
        "must NOT fall back to passthrough: {}",
        stdout(&o)
    );
    assert_eq!(env.profile_token(LIVE_EMAIL), "tok-mine");
    assert_eq!(env.live_token(), LIVE_TOKEN);
}

#[test]
fn login_rejects_a_key_that_is_neither_an_account_nor_a_profile() {
    let env = Env::new();
    for bad in ["work", "default", "off", "has space@x.com", "a/b@x.com"] {
        let o = env.login_as(bad, "who@x.com", "tok-a");
        assert!(!o.status.success(), "'{bad}' must be rejected");
        assert!(
            stderr(&o).contains("not an account email"),
            "'{bad}': {}",
            stderr(&o)
        );
    }
    assert!(!env.config_path().exists() || !env.config_text().contains("[[profile]]"));
}

#[test]
fn login_without_a_key_is_an_error_off_a_terminal() {
    let env = Env::new();
    let fake = env.fake_login("a@x.com", "tok-a");
    let o = env.cswap_env(&["login"], &[("CSWAP_CLAUDE_BIN", fake.to_str().unwrap())]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("pass the email"), "{}", stderr(&o));
}

#[test]
fn login_by_alias_relogs_in_the_existing_profile() {
    let env = Env::new();
    assert_ok(&env.login("work@x.com", "tok-1"));
    assert_ok(&env.cswap(&["alias", "create", "work@x.com", "w"]));

    let o = env.login_as("w", "work@x.com", "tok-2");
    assert_ok(&o);
    assert!(
        stdout(&o).contains("Logged work@x.com in again"),
        "{}",
        stdout(&o)
    );
    assert_eq!(env.profile_token("work@x.com"), "tok-2");
    assert_eq!(env.config_text().matches("[[profile]]").count(), 1);
}

// -------------------------------------------------------------------- running

#[test]
fn default_runs_passthrough_with_no_config_dir() {
    let env = Env::new();
    assert_ok(&env.login("work@x.com", "tok-work"));

    let o = env.run_cmd(&["run"], &[]);
    assert_ok(&o);
    assert!(stdout(&o).contains("CFG=\n"), "passthrough: {}", stdout(&o));

    let o = env.run_cmd(&["run", "default"], &[]);
    assert_ok(&o);
    assert!(stdout(&o).contains("CFG=\n"), "{}", stdout(&o));
    assert!(stderr(&o).contains("live ~/.claude"), "{}", stderr(&o));

    // A preset CLAUDE_CONFIG_DIR can't redirect what "the live ~/.claude" means.
    let o = env.run_cmd(&["run", "default"], &[("CLAUDE_CONFIG_DIR", "/tmp/hijack")]);
    assert_ok(&o);
    assert!(
        stdout(&o).contains("CFG=\n"),
        "must be scrubbed: {}",
        stdout(&o)
    );
}

#[test]
fn run_resolves_explicit_then_active_then_default() {
    let env = Env::new();
    assert_ok(&env.login("one@x.com", "tok-one"));
    assert_ok(&env.login("two@x.com", "tok-two"));
    assert_ok(&env.cswap(&["alias", "create", "one@x.com", "one"]));

    let one = format!("CFG={}", env.profile("one@x.com").display());
    let two = format!("CFG={}", env.profile("two@x.com").display());

    // CSWAP_ACTIVE picks a profile, by email or alias; explicit overrides it.
    for key in ["one@x.com", "one"] {
        let o = env.run_cmd(&["run"], &[("CSWAP_ACTIVE", key)]);
        assert!(stdout(&o).contains(&one), "{key}: {}", stdout(&o));
    }
    let o = env.run_cmd(&["run", "two@x.com"], &[("CSWAP_ACTIVE", "one@x.com")]);
    assert!(stdout(&o).contains(&two), "{}", stdout(&o));
    // `default` overrides an active profile too.
    let o = env.run_cmd(&["run", "default"], &[("CSWAP_ACTIVE", "one@x.com")]);
    assert!(stdout(&o).contains("CFG=\n"), "{}", stdout(&o));
    // CSWAP_ACTIVE=default means default, not a missing profile.
    let o = env.run_cmd(&["run"], &[("CSWAP_ACTIVE", "default")]);
    assert_ok(&o);
    assert!(stdout(&o).contains("CFG=\n"), "{}", stdout(&o));
    // A stale CSWAP_ACTIVE is an error, not a silent fallback.
    let o = env.run_cmd(&["run"], &[("CSWAP_ACTIVE", "ghost")]);
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("does not match any profile"),
        "{}",
        stderr(&o)
    );

    // Flags pass through; API keys are scrubbed.
    let o = env.run_cmd(
        &["run", "one", "--resume", "--model", "opus"],
        &[("ANTHROPIC_API_KEY", "sk-should-be-scrubbed")],
    );
    assert_ok(&o);
    assert!(
        stdout(&o).contains("ARGS=--resume --model opus"),
        "{}",
        stdout(&o)
    );
    assert!(stdout(&o).contains("KEY=scrubbed"), "{}", stdout(&o));

    // `run <flag>` with no target match passes the flag through.
    let o = env.run_cmd(&["run", "--version"], &[]);
    assert_ok(&o);
    assert!(stdout(&o).contains("ARGS=--version"));

    // The _claude shim never eats a leading target-shaped word.
    let o = env.run_cmd(&["_claude", "one", "-r"], &[]);
    assert_ok(&o);
    assert!(stdout(&o).contains("ARGS=one -r"), "{}", stdout(&o));
}

#[test]
fn run_on_a_profile_that_never_logged_in_says_what_to_do() {
    let env = Env::new();
    assert_ok(&env.login("work@x.com", "tok-work"));
    // Simulate the migration's output: a registered profile with no tokens.
    fs::remove_file(env.profile("work@x.com").join(".credentials.json")).unwrap();

    let o = env.run_cmd(&["run", "work@x.com"], &[]);
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("has no login yet") && stderr(&o).contains("cswap login work@x.com"),
        "{}",
        stderr(&o)
    );
}

// -------------------------------------------------------------------- default

#[test]
fn default_is_read_only_and_refuses_to_swap() {
    let env = Env::new();
    assert_ok(&env.login("work@x.com", "tok-work"));

    let o = env.cswap(&["default"]);
    assert_ok(&o);
    assert!(stdout(&o).contains(LIVE_EMAIL), "{}", stdout(&o));
    assert!(stdout(&o).contains("only reads it"), "{}", stdout(&o));

    let o = env.cswap(&["default", "work@x.com"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("no longer swaps"), "{}", stderr(&o));
    assert_eq!(env.live_token(), LIVE_TOKEN);
    assert_eq!(env.profile_token("work@x.com"), "tok-work");
}

// ------------------------------------------------------------------- activate

#[test]
fn activate_exports_the_account_and_unsets_for_default() {
    let env = Env::new();
    assert_ok(&env.login("work@x.com", "tok-work"));
    assert_ok(&env.cswap(&["alias", "create", "work@x.com", "w"]));

    // By email or alias, the export carries the stable key: the email.
    for key in ["work@x.com", "w"] {
        let o = env.cswap(&["activate", "--print", key]);
        assert_ok(&o);
        assert_eq!(stdout(&o).trim(), "export CSWAP_ACTIVE='work@x.com'");
    }
    for key in ["default", "off"] {
        let o = env.cswap(&["activate", "--print", key]);
        assert_ok(&o);
        assert_eq!(stdout(&o).trim(), "unset CSWAP_ACTIVE");
    }
    // A bare non-interactive activate means default, not "profile ''".
    let o = env.cswap(&["activate", "--print"]);
    assert_ok(&o);
    assert_eq!(stdout(&o).trim(), "unset CSWAP_ACTIVE");
    // Pre-0.5.1 wrappers passed an empty string; same meaning.
    let o = env.cswap(&["activate", "--print", ""]);
    assert_ok(&o);
    assert_eq!(stdout(&o).trim(), "unset CSWAP_ACTIVE");
    assert!(!env
        .cswap(&["activate", "--print", "ghost"])
        .status
        .success());

    // Without --print it explains the shell integration instead of guessing.
    let o = env.cswap(&["activate", "work@x.com"]);
    assert_ok(&o);
    assert!(stdout(&o).is_empty(), "nothing to eval: {}", stdout(&o));
    assert!(stderr(&o).contains("shell integration"), "{}", stderr(&o));
}

// ---------------------------------------------------------------------- alias

#[test]
fn alias_subcommands_create_list_remove() {
    let env = Env::new();
    assert_ok(&env.login("one@x.com", "tok-one"));

    assert_ok(&env.cswap(&["alias", "create", "one@x.com", "o1"]));
    let o = env.cswap(&["alias", "list"]);
    assert_ok(&o);
    assert!(
        stdout(&o).contains("one@x.com") && stdout(&o).contains("o1"),
        "{}",
        stdout(&o)
    );

    let o = env.cswap(&["activate", "--print", "o1"]);
    assert_eq!(stdout(&o).trim(), "export CSWAP_ACTIVE='one@x.com'");

    // An email or alias can't be reused, and `default` is not a profile.
    assert!(!env
        .cswap(&["alias", "create", "one@x.com", "o1"])
        .status
        .success());
    assert!(!env
        .cswap(&["alias", "create", "one@x.com", "one@x.com"])
        .status
        .success());
    let o = env.cswap(&["alias", "create", "default", "d"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("not a profile"), "{}", stderr(&o));

    assert_ok(&env.cswap(&["alias", "remove", "o1"]));
    assert!(!env.cswap(&["activate", "--print", "o1"]).status.success());
    assert!(!env.cswap(&["alias", "remove", "ghost"]).status.success());
}

// --------------------------------------------------------------------- remove

#[test]
fn remove_deletes_the_profile_and_spares_the_shared_data() {
    let env = Env::new();
    assert_ok(&env.login("work@x.com", "tok-work"));
    let profile = env.profile("work@x.com");
    assert!(profile.is_dir());

    // Non-interactive without --yes: refuse rather than silently clobber.
    let o = env.cswap(&["remove", "work@x.com"]);
    assert!(!o.status.success(), "must not remove: {}", stdout(&o));
    assert!(profile.is_dir(), "still there");

    assert_ok(&env.cswap(&["remove", "work@x.com", "--yes"]));
    assert!(!profile.exists(), "profile dir gone");
    assert!(!env.config_text().contains("[[profile]]"), "entry gone");

    // The symlink targets survived: remove never follows a link.
    for item in ["settings.json", "CLAUDE.md", "history.jsonl"] {
        assert!(
            env.live().join(item).exists(),
            "~/.claude/{item} must survive"
        );
    }
    assert!(env.live().join("projects/proj-a/s1.jsonl").exists());
    assert!(env.live().join("plugins/repos").is_dir());
    assert_eq!(env.live_token(), LIVE_TOKEN);
}

#[test]
fn remove_refuses_the_default() {
    let env = Env::new();
    let o = env.cswap(&["remove", "default", "--yes"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("nothing to remove"), "{}", stderr(&o));
    assert_eq!(env.live_token(), LIVE_TOKEN);
}

#[test]
fn removing_a_profile_leaves_the_shared_account_as_the_default() {
    let env = Env::new();
    assert_ok(&env.login(LIVE_EMAIL, "tok-mine"));
    let o = env.cswap(&["remove", LIVE_EMAIL, "--yes"]);
    assert_ok(&o);
    assert!(
        stdout(&o).contains("still the default"),
        "the separation is stated: {}",
        stdout(&o)
    );
    assert_eq!(env.live_token(), LIVE_TOKEN);
}

// ------------------------------------------------------------------ list/usage

#[test]
fn list_shows_the_default_line_then_one_row_per_profile() {
    let env = Env::new();
    assert_ok(&env.login("one@x.com", "tok-one"));
    assert_ok(&env.login("two@x.com", "tok-two"));
    assert_ok(&env.cswap(&["alias", "create", "one@x.com", "o"]));

    let o = env.cswap(&["list", "--quick"]);
    assert_ok(&o);
    let out = stdout(&o);
    let first = out.lines().next().unwrap();
    assert!(first.starts_with("default"), "default first: {out}");
    assert!(first.contains(LIVE_EMAIL), "live login: {out}");
    assert!(first.contains("live ~/.claude"), "{out}");
    assert!(first.contains("● active"), "default in effect: {out}");
    assert!(out.contains("ACCOUNT"), "header: {out}");

    let row = |email: &str| {
        out.lines()
            .find(|l| l.contains(email) && !l.starts_with("default"))
            .unwrap_or_else(|| panic!("no row for {email}: {out}"))
    };
    assert!(row("one@x.com").contains(" o "), "alias column: {out}");
    assert!(
        row("two@x.com").trim_start().starts_with("two@x.com"),
        "no status word while nothing is active: {out}"
    );

    // Activating a profile moves the marker off the default.
    let o = env.cswap_env(&["list", "--quick"], &[("CSWAP_ACTIVE", "one@x.com")]);
    let out = stdout(&o);
    assert!(
        !out.lines().next().unwrap().contains("● active"),
        "default not in effect: {out}"
    );
    let row = out
        .lines()
        .find(|l| l.contains("one@x.com") && !l.starts_with("default"))
        .unwrap();
    assert!(row.starts_with("active"), "{out}");
}

#[test]
fn usage_renders_a_card_for_the_default_and_each_profile() {
    let env = Env::new();
    assert_ok(&env.login("one@x.com", "tok-one"));
    assert_ok(&env.login("two@x.com", "tok-two"));
    assert_ok(&env.cswap(&["alias", "create", "one@x.com", "o"]));

    // Port 1 refuses instantly: exercises the error path without a network.
    let dead = [("CSWAP_USAGE_URL", "http://127.0.0.1:1/usage")];
    let o = env.cswap_env(&["usage"], &dead);
    assert_ok(&o);
    let out = stdout(&o);
    let first = out.lines().next().unwrap();
    assert!(first.starts_with("default —"), "default card first: {out}");
    assert!(first.contains("[live ~/.claude]"), "{out}");
    assert!(first.contains("● active"), "{out}");
    assert!(
        out.contains("one@x.com") && out.contains("two@x.com"),
        "{out}"
    );
    assert!(out.contains("[o]"), "aliases in the header: {out}");
    assert_eq!(out.matches("usage unavailable").count(), 3, "{out}");

    // Scoped to one profile hides the other AND the default card.
    let o = env.cswap_env(&["usage", "two@x.com"], &dead);
    assert_ok(&o);
    let out = stdout(&o);
    assert!(!out.contains("one@x.com"), "{out}");
    assert!(!out.contains("default —"), "{out}");

    // An alias scopes to its account.
    let o = env.cswap_env(&["usage", "o"], &dead);
    assert_ok(&o);
    assert!(stdout(&o).contains("one@x.com"), "{}", stdout(&o));
    assert!(!stdout(&o).contains("two@x.com"), "{}", stdout(&o));

    // Scoped to the default hides every profile.
    let o = env.cswap_env(&["usage", "default"], &dead);
    assert_ok(&o);
    let out = stdout(&o);
    assert!(out.contains("default —"), "{out}");
    assert!(!out.contains("one@x.com"), "{out}");

    assert!(!env.cswap_env(&["usage", "ghost"], &dead).status.success());
}

// ------------------------------------------------------------------- migration

#[test]
fn migrates_a_0_5_account_config_without_moving_anything() {
    let env = Env::new();
    fs::create_dir_all(env.config_path().parent().unwrap()).unwrap();
    fs::write(
        env.config_path(),
        "[[account]]\nemail = \"dev@neura.org\"\naliases = [\"neura\", \"1\"]\n\n\
         [[account]]\nemail = \"me@gmail.com\"\n",
    )
    .unwrap();
    // 0.5 already keyed directories by email, so this one must not move.
    let dir = env.profile("dev@neura.org");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(".credentials.json"),
        Env::creds("tok-neura").to_string(),
    )
    .unwrap();
    let store = env.data().join("accounts");
    fs::create_dir_all(&store).unwrap();
    fs::write(store.join("me@gmail.com.creds.json"), "{}").unwrap();

    let o = env.cswap(&["list", "--quick"]);
    assert_ok(&o);
    assert!(
        stderr(&o).contains("migrated to the 0.6.1 model"),
        "{}",
        stderr(&o)
    );

    let cfg = env.config_text();
    assert!(!cfg.contains("[[account]]"), "{cfg}");
    assert!(cfg.contains("email = \"dev@neura.org\""), "{cfg}");
    assert!(
        cfg.contains("\"neura\"") && cfg.contains("\"1\""),
        "aliases kept: {cfg}"
    );

    // The directory never moved, so nothing recorded in ~/.claude can dangle.
    assert!(dir.is_dir(), "the email-keyed directory stays put");
    assert_eq!(env.profile_token("dev@neura.org"), "tok-neura");
    assert!(
        !stderr(&o).contains("back under its email"),
        "nothing to move from 0.5: {}",
        stderr(&o)
    );

    // The old store moved aside rather than being merged into a profile.
    assert!(!store.exists());
    assert!(env.data().join("accounts.pre-0.6.bak").is_dir());
    assert!(
        !env.profile("me@gmail.com")
            .join(".credentials.json")
            .exists(),
        "no credential was copied into a profile"
    );
    assert!(
        stderr(&o).contains("cswap login me@gmail.com"),
        "{}",
        stderr(&o)
    );

    // Idempotent.
    let o2 = env.cswap(&["list", "--quick"]);
    assert_ok(&o2);
    assert!(!stderr(&o2).contains("migrated"), "{}", stderr(&o2));
}

/// 0.6.0 briefly keyed profiles by a separate `name`, which renamed the
/// directory. Undo that: the name becomes an alias and the directory goes back
/// under the account.
#[test]
fn migrates_a_0_6_0_named_config_back_under_the_account() {
    let env = Env::new();
    fs::create_dir_all(env.config_path().parent().unwrap()).unwrap();
    fs::write(
        env.config_path(),
        "[[profile]]\nname = \"wadhwani\"\nemail = \"dev@gmail.com\"\n",
    )
    .unwrap();
    let named = env.profile("wadhwani");
    fs::create_dir_all(&named).unwrap();
    fs::write(
        named.join(".credentials.json"),
        Env::creds("tok-w").to_string(),
    )
    .unwrap();
    // 0.6.0 may also have left a compatibility symlink at the email path.
    std::os::unix::fs::symlink(&named, env.profile("dev@gmail.com")).unwrap();

    let o = env.cswap(&["list", "--quick"]);
    assert_ok(&o);
    assert!(
        stderr(&o).contains("back under its email"),
        "{}",
        stderr(&o)
    );

    let cfg = env.config_text();
    assert!(!cfg.contains("name ="), "the name key is gone: {cfg}");
    assert!(cfg.contains("email = \"dev@gmail.com\""), "{cfg}");
    assert!(
        cfg.contains("aliases = [\"wadhwani\"]"),
        "the name survives as an alias: {cfg}"
    );

    // The directory is back under the account, and the stale link is gone.
    assert!(!named.exists(), "name-keyed directory removed");
    let dir = env.profile("dev@gmail.com");
    assert!(
        dir.is_dir() && !is_symlink(&dir),
        "real directory, not a link"
    );
    assert_eq!(env.profile_token("dev@gmail.com"), "tok-w");

    // The alias still resolves everywhere.
    let o = env.cswap(&["activate", "--print", "wadhwani"]);
    assert_ok(&o);
    assert_eq!(stdout(&o).trim(), "export CSWAP_ACTIVE='dev@gmail.com'");
}

#[test]
fn migration_survives_a_pre_0_4_name_and_a_stored_default() {
    let env = Env::new();
    fs::create_dir_all(env.config_path().parent().unwrap()).unwrap();
    fs::write(
        env.config_path(),
        "default = \"main\"\n\n[[account]]\nname = \"main\"\nemail = \"m@x.com\"\n\
         isolated = true\n",
    )
    .unwrap();

    assert_ok(&env.cswap(&["list", "--quick"]));
    let cfg = env.config_text();
    assert!(cfg.contains("email = \"m@x.com\""), "{cfg}");
    assert!(
        cfg.contains("aliases = [\"main\"]"),
        "the old name became an alias: {cfg}"
    );
    assert!(
        !cfg.contains("default ="),
        "no stored default survives: {cfg}"
    );
    assert!(!cfg.contains("isolated"), "obsolete field dropped: {cfg}");
}

// ------------------------------------------------------------- sync behaviour

#[test]
fn profile_sync_picks_up_new_files_and_prunes_dangling_links() {
    let env = Env::new();
    assert_ok(&env.login("one@x.com", "tok-one"));
    let profile = env.profile("one@x.com");
    let src = env.live();

    fs::write(src.join("brand-new.json"), "{}").unwrap();
    assert_ok(&env.run_cmd(&["run", "one@x.com"], &[]));
    assert!(is_link_to(
        &profile.join("brand-new.json"),
        &src.join("brand-new.json")
    ));

    fs::remove_file(src.join("brand-new.json")).unwrap();
    assert_ok(&env.run_cmd(&["run", "one@x.com"], &[]));
    assert!(!profile.join("brand-new.json").exists());

    // A real file the profile grew on its own is never clobbered.
    fs::write(profile.join("settings.json.local"), "mine").unwrap();
    fs::write(src.join("settings.json.local"), "theirs").unwrap();
    assert_ok(&env.run_cmd(&["run", "one@x.com"], &[]));
    assert_eq!(
        fs::read_to_string(profile.join("settings.json.local")).unwrap(),
        "mine"
    );
}

/// A profile built by an older cswap carries links for names that have since
/// joined the denylist. sync_links must drop them on the next launch instead of
/// leaving a `.git` that makes git treat the profile as ~/.claude's worktree.
#[test]
fn run_prunes_links_that_have_since_been_denylisted() {
    let env = Env::new();
    assert_ok(&env.login("one@x.com", "tok-one"));
    let profile = env.profile("one@x.com");
    let src = env.live();

    for item in ["backups", ".git"] {
        std::os::unix::fs::symlink(src.join(item), profile.join(item)).unwrap();
        assert!(is_link_to(&profile.join(item), &src.join(item)));
    }

    assert_ok(&env.run_cmd(&["run", "one@x.com"], &[]));

    for item in ["backups", ".git"] {
        assert!(
            !profile.join(item).exists(),
            "{item} link should have been pruned"
        );
    }
    for item in ["backups/.claude.json.backup.1", ".git/refs"] {
        assert!(src.join(item).exists(), "~/.claude/{item} must survive");
    }
}

// ----------------------------------------------------------------- shell init

#[test]
fn shell_init_emits_wrappers() {
    let env = Env::new();
    for shell in ["bash", "zsh"] {
        let o = env.cswap(&["shell-init", shell]);
        assert_ok(&o);
        let out = stdout(&o);
        assert!(out.contains("cswap()"), "{out}");
        assert!(out.contains("claude()"), "{out}");
        assert!(out.contains("cswap _claude"), "{out}");
    }
    assert!(!env.cswap(&["shell-init", "fish"]).status.success());
}
