//! End-to-end tests against the real binary, inside a fabricated $HOME.
//!
//! Nothing here touches the developer's actual ~/.claude: every invocation
//! gets HOME/XDG_* pointed at a TempDir, and credentials carry a far-future
//! expiresAt so no code path ever reaches the network. All runs are
//! non-interactive (no tty), so interactive pickers are never entered —
//! commands take the argument/fallback paths.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_cswap");

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
        fs::write(
            claude.join(".credentials.json"),
            Self::creds("tok-live").to_string(),
        )
        .unwrap();
        fs::write(
            h.join(".claude.json"),
            json!({
                "oauthAccount": {"emailAddress": "one@x.com", "accountUuid": "u-1"},
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
            "refreshToken": "refresh-1",
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

    fn profile(&self, email: &str) -> PathBuf {
        self.data().join("profiles").join(email)
    }

    /// Point the live login at a different account (new email + token).
    fn switch_live_account(&self, email: &str, token: &str) {
        let h = self.home.path();
        fs::write(
            h.join(".claude/.credentials.json"),
            Self::creds(token).to_string(),
        )
        .unwrap();
        let mut cfg: Value =
            serde_json::from_str(&fs::read_to_string(h.join(".claude.json")).unwrap()).unwrap();
        cfg["oauthAccount"] = json!({"emailAddress": email, "accountUuid": "u-x"});
        fs::write(h.join(".claude.json"), cfg.to_string()).unwrap();
    }

    /// Make the live ~/.claude belong to nobody cswap knows, so registered
    /// accounts exercise the profile path (not live passthrough).
    fn detach_live(&self) {
        self.switch_live_account("nobody@x.com", "tok-live");
    }

    /// A fake `claude` that prints its CLAUDE_CONFIG_DIR and args, then exits.
    fn fake_claude(&self) -> PathBuf {
        let path = self.home.path().join("fake-claude.sh");
        fs::write(&path, "#!/bin/sh\necho \"CFG=$CLAUDE_CONFIG_DIR\"\necho \"ARGS=$*\"\necho \"KEY=${ANTHROPIC_API_KEY:-scrubbed}\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
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

#[test]
fn login_registers_by_email_and_sets_default() {
    let env = Env::new();
    let o = env.cswap(&["login"]);
    assert_ok(&o);
    assert!(stdout(&o).contains("Registered one@x.com"));
    assert!(stdout(&o).contains("default"));

    let cfg = fs::read_to_string(env.config_path()).unwrap();
    assert!(cfg.contains("default = \"one@x.com\""));
    assert!(cfg.contains("email = \"one@x.com\""));
    assert!(!cfg.contains("name ="), "no name concept in config");
    assert!(env.data().join("accounts/one@x.com.creds.json").exists());
    assert!(env.data().join("accounts/one@x.com.meta.json").exists());

    // Same email again = relogin, not a duplicate — and it says who loudly.
    let o = env.cswap(&["login"]);
    assert_ok(&o);
    assert!(stdout(&o).contains("Live ~/.claude login: one@x.com"));
    assert!(stdout(&o).contains("Refreshed stored credentials"));
    let cfg = fs::read_to_string(env.config_path()).unwrap();
    assert!(cfg.contains("one@x.com"));
    assert_eq!(
        cfg.matches("[[account]]").count(),
        1,
        "no duplicate account: {cfg}"
    );
}

#[test]
fn login_alias_flag_registers_and_extends() {
    let env = Env::new();
    // Register with an alias attached immediately.
    let o = env.cswap(&["login", "--alias", "main"]);
    assert_ok(&o);
    assert!(stdout(&o).contains("Registered one@x.com"));
    assert!(stdout(&o).contains("'main' now points to one@x.com"));

    // Relogin with a second alias adds it to the same account.
    let o = env.cswap(&["login", "--alias", "m2"]);
    assert_ok(&o);
    assert!(stdout(&o).contains("'m2' now points to one@x.com"));

    // Duplicate alias is rejected.
    let o = env.cswap(&["login", "--alias", "main"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("already used"));

    // Invalid alias is rejected.
    let o = env.cswap(&["login", "--alias", "Bad Name"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("invalid alias"));
}

#[test]
fn login_without_live_credentials_fails_helpfully() {
    let env = Env::new();
    fs::remove_file(env.home.path().join(".claude/.credentials.json")).unwrap();
    let o = env.cswap(&["login"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("log in first"));
}

#[test]
fn login_new_captures_fresh_account_via_staging() {
    let env = Env::new();
    assert_ok(&env.cswap(&["login", "--alias", "main"]));

    // Fake claude that "logs in" as wadhwani inside $CLAUDE_CONFIG_DIR.
    let fake = env.home.path().join("fake-login.sh");
    fs::write(
        &fake,
        r#"#!/bin/sh
cat > "$CLAUDE_CONFIG_DIR/.credentials.json" <<'EOF'
{"claudeAiOauth":{"accessToken":"tok-wad","refreshToken":"r-wad","expiresAt":9999999999999}}
EOF
cat > "$CLAUDE_CONFIG_DIR/.claude.json" <<'EOF'
{"hasCompletedOnboarding":true,"theme":"dark","oauthAccount":{"emailAddress":"wadhwani@x.com"}}
EOF
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();

    let o = env.cswap_env(
        &["login", "--new", "--alias", "wadhwani"],
        &[("CSWAP_CLAUDE_BIN", fake.to_str().unwrap())],
    );
    assert_ok(&o);
    assert!(stdout(&o).contains("Registered wadhwani@x.com"));
    assert!(stdout(&o).contains("'wadhwani' now points to wadhwani@x.com"));

    // Live login untouched; staging promoted to a real email-keyed profile.
    let live: Value = serde_json::from_str(
        &fs::read_to_string(env.home.path().join(".claude/.credentials.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(live["claudeAiOauth"]["accessToken"], json!("tok-live"));
    assert!(env
        .profile("wadhwani@x.com")
        .join(".credentials.json")
        .exists());
    assert!(!env.data().join("staging-login").exists());

    // Logging into an ALREADY-registered account via --new is caught too.
    let o = env.cswap_env(
        &["login", "--new"],
        &[("CSWAP_CLAUDE_BIN", fake.to_str().unwrap())],
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("already registered"));
}

#[test]
fn default_and_activate_resolve_alias_and_email() {
    let env = Env::new();
    assert_ok(&env.cswap(&["login", "--alias", "one"]));
    env.switch_live_account("two@x.com", "tok-two");
    assert_ok(&env.cswap(&["login", "--alias", "two"]));

    // default: show (piped -> just prints) + set by alias + set by email
    let o = env.cswap(&["default"]);
    assert!(stdout(&o).contains("default: one@x.com"));
    assert_ok(&env.cswap(&["default", "two"]));
    assert!(stdout(&env.cswap(&["default"])).contains("default: two@x.com"));
    assert_ok(&env.cswap(&["default", "one@x.com"]));
    assert!(stdout(&env.cswap(&["default"])).contains("default: one@x.com"));

    // activate --print emits eval-able lines; export carries the EMAIL.
    let o = env.cswap(&["activate", "--print", "two"]);
    assert_ok(&o);
    assert_eq!(stdout(&o).trim(), "export CSWAP_ACTIVE='two@x.com'");
    let o = env.cswap(&["activate", "--print", "two@x.com"]);
    assert_eq!(stdout(&o).trim(), "export CSWAP_ACTIVE='two@x.com'");
    let o = env.cswap(&["activate", "--print", "default"]);
    assert_eq!(stdout(&o).trim(), "unset CSWAP_ACTIVE");
    let o = env.cswap(&["activate", "--print"]); // piped: falls back to default
    assert_eq!(stdout(&o).trim(), "unset CSWAP_ACTIVE");
    let o = env.cswap(&["activate", "--print", "ghost"]);
    assert!(!o.status.success());
}

#[test]
fn run_builds_email_keyed_profile_and_execs_claude() {
    let env = Env::new();
    assert_ok(&env.cswap(&["login", "--alias", "one"]));
    env.detach_live(); // 'one@x.com' is no longer the live login -> profile path
    let fake = env.fake_claude();

    let o = env.cswap_env(
        &["run", "one", "--resume", "--model", "opus"],
        &[
            ("CSWAP_CLAUDE_BIN", fake.to_str().unwrap()),
            ("ANTHROPIC_API_KEY", "sk-should-be-scrubbed"),
        ],
    );
    assert_ok(&o);
    let out = stdout(&o);
    let profile = env.profile("one@x.com");
    assert!(out.contains(&format!("CFG={}", profile.display())));
    assert!(out.contains("ARGS=--resume --model opus"));
    assert!(
        out.contains("KEY=scrubbed"),
        "API key must be scrubbed: {out}"
    );

    // Identity: real files, private perms.
    use std::os::unix::fs::PermissionsExt;
    for f in [".credentials.json", ".claude.json"] {
        let p = profile.join(f);
        let md = fs::symlink_metadata(&p).unwrap();
        assert!(md.file_type().is_file(), "{f} must be a real file");
        assert_eq!(md.permissions().mode() & 0o777, 0o600, "{f} must be 0600");
    }

    // Seeded .claude.json: onboarding skip + identity + carried keys.
    let cj: Value =
        serde_json::from_str(&fs::read_to_string(profile.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(cj["hasCompletedOnboarding"], json!(true));
    assert_eq!(cj["oauthAccount"]["emailAddress"], json!("one@x.com"));
    assert!(cj["mcpServers"]["srv"].is_object());
    assert_eq!(
        cj["projects"]["/tmp/repo"]["hasTrustDialogAccepted"],
        json!(true)
    );

    // Everything else: symlinks into ~/.claude (share-all-except-denylist).
    let src = env.home.path().join(".claude");
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
}

#[test]
fn run_resolves_active_then_default_and_shim_never_eats_args() {
    let env = Env::new();
    assert_ok(&env.cswap(&["login", "--alias", "one"]));
    env.switch_live_account("two@x.com", "tok-two");
    assert_ok(&env.cswap(&["login", "--alias", "two"]));
    env.detach_live(); // neither account is the live login
    let fake = env.fake_claude();
    let bin = fake.to_str().unwrap();

    // No name -> default (one@x.com). Piped stdin => no interactive picker.
    let o = env.cswap_env(&["run"], &[("CSWAP_CLAUDE_BIN", bin)]);
    assert_ok(&o);
    assert!(stdout(&o).contains(&format!("CFG={}", env.profile("one@x.com").display())));

    // CSWAP_ACTIVE (alias form) overrides default.
    let o = env.cswap_env(
        &["run"],
        &[("CSWAP_CLAUDE_BIN", bin), ("CSWAP_ACTIVE", "two")],
    );
    assert_ok(&o);
    assert!(stdout(&o).contains(&format!("CFG={}", env.profile("two@x.com").display())));

    // `run <flag>` with no account match passes the flag through.
    let o = env.cswap_env(&["run", "--version"], &[("CSWAP_CLAUDE_BIN", bin)]);
    assert_ok(&o);
    assert!(stdout(&o).contains("ARGS=--version"));

    // The _claude shim passes even account-shaped words through verbatim.
    let o = env.cswap_env(&["_claude", "two", "-r"], &[("CSWAP_CLAUDE_BIN", bin)]);
    assert_ok(&o);
    assert!(stdout(&o).contains("ARGS=two -r"));
    assert!(stdout(&o).contains(&format!("CFG={}", env.profile("one@x.com").display())));
}

#[test]
fn live_account_runs_passthrough_without_profile() {
    let env = Env::new();
    assert_ok(&env.cswap(&["login", "--alias", "one"]));
    let fake = env.fake_claude();
    let o = env.cswap_env(
        &["run", "one", "-r"],
        &[
            ("CSWAP_CLAUDE_BIN", fake.to_str().unwrap()),
            ("CLAUDE_CONFIG_DIR", "/should/be/removed"),
        ],
    );
    assert_ok(&o);
    let out = stdout(&o);
    assert!(
        out.contains("CFG=\n"),
        "passthrough must unset CLAUDE_CONFIG_DIR: {out}"
    );
    assert!(!env.profile("one@x.com").exists());
    assert!(stderr(&o).contains("[live ~/.claude]"));
}

#[test]
fn alias_subcommands_create_list_remove() {
    let env = Env::new();
    assert_ok(&env.cswap(&["login", "--alias", "one"]));
    env.detach_live();

    assert_ok(&env.cswap(&["alias", "create", "one@x.com", "o1"]));
    // Alias resolves in activate / default / run.
    let o = env.cswap(&["activate", "--print", "o1"]);
    assert_ok(&o);
    assert_eq!(stdout(&o).trim(), "export CSWAP_ACTIVE='one@x.com'");
    assert_ok(&env.cswap(&["default", "o1"]));

    let fake = env.fake_claude();
    let o = env.cswap_env(
        &["run", "o1"],
        &[("CSWAP_CLAUDE_BIN", fake.to_str().unwrap())],
    );
    assert_ok(&o);
    assert!(stdout(&o).contains(&format!("CFG={}", env.profile("one@x.com").display())));

    // list shows both labels.
    let o = env.cswap(&["alias", "list"]);
    assert_ok(&o);
    let out = stdout(&o);
    assert!(out.contains("one@x.com"));
    assert!(out.contains("one, o1"));

    // Uniqueness across aliases and emails.
    assert!(!env
        .cswap(&["alias", "create", "one@x.com", "o1"])
        .status
        .success());
    assert!(!env
        .cswap(&["alias", "create", "one@x.com", "one@x.com"])
        .status
        .success());

    // Remove; alias stops resolving.
    assert_ok(&env.cswap(&["alias", "remove", "o1"]));
    assert!(!env.cswap(&["activate", "--print", "o1"]).status.success());
    // Removing a nonexistent alias fails.
    assert!(!env.cswap(&["alias", "remove", "ghost"]).status.success());
}

#[test]
fn list_shows_default_entity_line_and_markers() {
    let env = Env::new();
    assert_ok(&env.cswap(&["login", "--alias", "one"]));
    env.switch_live_account("two@x.com", "tok-two");
    assert_ok(&env.cswap(&["login", "--alias", "two"]));

    let o = env.cswap_env(&["list", "--quick"], &[("CSWAP_ACTIVE", "two")]);
    assert_ok(&o);
    let out = stdout(&o);
    // Default/Active are standalone lines with the EMAIL only.
    assert!(out.contains("Default: one@x.com"), "got: {out}");
    assert!(out.contains("Active:  two@x.com"), "got: {out}");
    // Table rows: alias column + email, with d/* markers.
    let rows: Vec<&str> = out
        .lines()
        .filter(|l| !l.starts_with("Default:") && !l.starts_with("Active:"))
        .collect();
    let one_line = rows.iter().find(|l| l.contains("one@x.com")).unwrap();
    let two_line = rows.iter().find(|l| l.contains("two@x.com")).unwrap();
    assert!(
        one_line.trim_start().starts_with("d "),
        "one is default: {one_line}"
    );
    assert!(
        two_line.trim_start().starts_with("* "),
        "two is active: {two_line}"
    );
}

#[test]
fn remove_requires_confirmation_and_spares_claude_data() {
    let env = Env::new();
    assert_ok(&env.cswap(&["login", "--alias", "one"]));
    env.detach_live();
    let fake = env.fake_claude();
    assert_ok(&env.cswap_env(
        &["run", "one"],
        &[("CSWAP_CLAUDE_BIN", fake.to_str().unwrap())],
    ));
    assert!(env.profile("one@x.com").exists());

    // Piped + no --yes: refuses rather than deleting silently.
    let o = env.cswap(&["remove", "one"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--yes"));
    assert!(
        env.profile("one@x.com").exists(),
        "nothing deleted without consent"
    );

    let o = env.cswap(&["remove", "one", "--yes"]);
    assert_ok(&o);
    assert!(!env.profile("one@x.com").exists());
    assert!(!env.data().join("accounts/one@x.com.creds.json").exists());

    // THE guarantee: removing a profile full of symlinks must not touch the
    // real ~/.claude data those links pointed at.
    let src = env.home.path().join(".claude");
    assert!(src.join("projects/proj-a/s1.jsonl").exists());
    assert!(src.join("history.jsonl").exists());
    assert!(src.join("settings.json").exists());
    assert!(src.join("CLAUDE.md").exists());
}

#[test]
fn legacy_name_config_migrates_on_disk() {
    let env = Env::new();
    // Fabricate a pre-0.4 layout: name-keyed config + store files + profile.
    fs::create_dir_all(env.config_path().parent().unwrap()).unwrap();
    fs::write(
        env.config_path(),
        "default = \"main\"\n\n[[account]]\nname = \"main\"\nemail = \"one@x.com\"\n",
    )
    .unwrap();
    fs::create_dir_all(env.data().join("accounts")).unwrap();
    fs::write(
        env.data().join("accounts/main.creds.json"),
        Env::creds("t").to_string(),
    )
    .unwrap();
    fs::write(env.data().join("accounts/main.meta.json"), "{}").unwrap();
    fs::create_dir_all(env.data().join("profiles/main")).unwrap();
    fs::write(env.data().join("profiles/main/.credentials.json"), "{}").unwrap();

    // Any command triggers the migration.
    let o = env.cswap(&["list", "--quick"]);
    assert_ok(&o);
    assert!(stderr(&o).contains("migrated"), "announces the migration");

    let cfg = fs::read_to_string(env.config_path()).unwrap();
    assert!(
        cfg.contains("default = \"one@x.com\""),
        "default canonicalized: {cfg}"
    );
    assert!(
        cfg.contains("aliases = [\"main\"]"),
        "name became alias: {cfg}"
    );
    assert!(!cfg.contains("name ="), "no name field survives: {cfg}");
    assert!(env.data().join("accounts/one@x.com.creds.json").exists());
    assert!(env.data().join("accounts/one@x.com.meta.json").exists());
    assert!(env
        .data()
        .join("profiles/one@x.com/.credentials.json")
        .exists());
    assert!(!env.data().join("profiles/main").exists());

    // The old name keeps working — it's an alias now.
    let o = env.cswap(&["activate", "--print", "main"]);
    assert_ok(&o);
    assert_eq!(stdout(&o).trim(), "export CSWAP_ACTIVE='one@x.com'");
}

#[test]
fn profile_sync_picks_up_new_files_and_prunes_dangling() {
    let env = Env::new();
    assert_ok(&env.cswap(&["login", "--alias", "one"]));
    env.detach_live();
    let fake = env.fake_claude();
    let bin = fake.to_str().unwrap();
    assert_ok(&env.cswap_env(&["run", "one"], &[("CSWAP_CLAUDE_BIN", bin)]));

    let src = env.home.path().join(".claude");
    let profile = env.profile("one@x.com");

    fs::write(src.join("future-invention.json"), "{}").unwrap();
    fs::remove_file(src.join("CLAUDE.md")).unwrap();

    assert_ok(&env.cswap_env(&["run", "one"], &[("CSWAP_CLAUDE_BIN", bin)]));
    assert!(is_link_to(
        &profile.join("future-invention.json"),
        &src.join("future-invention.json")
    ));
    assert!(
        fs::symlink_metadata(profile.join("CLAUDE.md")).is_err(),
        "dangling link must be pruned"
    );
}

#[test]
fn isolated_account_gets_no_history_links() {
    let env = Env::new();
    assert_ok(&env.cswap(&["login", "--alias", "work"]));
    env.detach_live();
    let cfg = fs::read_to_string(env.config_path())
        .unwrap()
        .replace("isolated = false", "isolated = true");
    fs::write(env.config_path(), cfg).unwrap();

    let fake = env.fake_claude();
    assert_ok(&env.cswap_env(
        &["run", "work"],
        &[("CSWAP_CLAUDE_BIN", fake.to_str().unwrap())],
    ));

    let profile = env.profile("one@x.com");
    assert!(fs::symlink_metadata(profile.join("projects")).is_err());
    assert!(fs::symlink_metadata(profile.join("history.jsonl")).is_err());
    assert!(is_link_to(
        &profile.join("settings.json"),
        &env.home.path().join(".claude/settings.json")
    ));
}

#[test]
fn shell_init_emits_wrappers() {
    let env = Env::new();
    for shell in ["zsh", "bash"] {
        let o = env.cswap(&["shell-init", shell]);
        assert_ok(&o);
        let out = stdout(&o);
        assert!(out.contains("cswap() {"));
        assert!(out.contains("claude() {"));
        assert!(out.contains("_claude"));
        assert!(out.contains("activate --print"));
    }
    let o = env.cswap(&["shell-init", "powershell"]);
    assert!(!o.status.success());
}

#[test]
fn relogin_updates_existing_profile_credentials() {
    let env = Env::new();
    assert_ok(&env.cswap(&["login", "--alias", "one"]));
    env.detach_live();
    let fake = env.fake_claude();
    assert_ok(&env.cswap_env(
        &["run", "one"],
        &[("CSWAP_CLAUDE_BIN", fake.to_str().unwrap())],
    ));

    // Fresh login on the same email must overwrite the profile's copy too.
    env.switch_live_account("one@x.com", "tok-fresh");
    assert_ok(&env.cswap(&["login"]));
    let creds: Value = serde_json::from_str(
        &fs::read_to_string(env.profile("one@x.com").join(".credentials.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(creds["claudeAiOauth"]["accessToken"], json!("tok-fresh"));
}
