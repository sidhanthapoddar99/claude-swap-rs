//! End-to-end tests of the real binary, inside a fabricated `$HOME`.
//!
//! Nothing here touches the developer's own logins: every run gets `HOME` and
//! `CSWAP_HOME` pointed at a temp dir, the agents are shell scripts that log
//! what they were started with, and `CSWAP_OFFLINE` keeps every code path off
//! the network. No run has a terminal, so no picker is ever entered.
#![cfg(unix)]

use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_cswap");

/// A stand-in for `claude`, `codex`, `agy` and `grok`. It logs its arguments
/// one per line, so a test can tell `a b` from `a` `b`, and its sign-in
/// commands write a login the way the real ones do.
const FAKE_AGENT: &str = r#"#!/bin/sh
name=$(basename "$0")
{
  echo "run: $name"
  for a in "$@"; do echo "arg: $a"; done
  echo "CLAUDE_CONFIG_DIR=${CLAUDE_CONFIG_DIR:-}"
  echo "CODEX_HOME=${CODEX_HOME:-}"
  echo "CSWAP_ACCOUNT=${CSWAP_ACCOUNT:-}"
} >> "$CSWAP_FAKE_LOG"
case "$name $*" in
  "claude auth login")
    printf '%s' '{"claudeAiOauth":{"accessToken":"tok-new","refreshToken":"r","expiresAt":4102444800000}}' > "$CLAUDE_CONFIG_DIR/.credentials.json"
    printf '{"oauthAccount":{"emailAddress":"%s"}}' "${CSWAP_FAKE_EMAIL:-new@example.com}" > "$CLAUDE_CONFIG_DIR/.claude.json" ;;
  "codex login")
    if [ -n "${CSWAP_FAKE_FAIL:-}" ]; then echo "login cancelled"; exit 1; fi
    printf '{"tokens":{"access_token":"a.b.c","account_id":"%s"}}' "${CSWAP_FAKE_ACCT:-acct-new}" > "$CODEX_HOME/auth.json" ;;
  *) echo "$name ran" ;;
esac
exit "${CSWAP_FAKE_EXIT:-0}"
"#;

struct Env {
    home: TempDir,
}

impl Env {
    /// A home where Claude Code and Codex are installed and signed in, with a
    /// few shared files in each agent's own config dir.
    fn new() -> Env {
        let home = TempDir::new().unwrap();
        let h = home.path();

        let bin = h.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for name in ["claude", "codex"] {
            let path = bin.join(name);
            fs::write(&path, FAKE_AGENT).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::create_dir_all(h.join("proc")).unwrap();

        let claude = h.join(".claude");
        fs::create_dir_all(claude.join("projects/proj-a")).unwrap();
        fs::create_dir_all(claude.join("skills")).unwrap();
        fs::write(claude.join("settings.json"), r#"{"model":"opus"}"#).unwrap();
        fs::write(claude.join("CLAUDE.md"), "# rules\n").unwrap();
        fs::write(
            claude.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"tok-own","refreshToken":"r","expiresAt":4102444800000}}"#,
        )
        .unwrap();
        fs::write(
            h.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"own@example.com"}}"#,
        )
        .unwrap();

        let codex = h.join(".codex");
        fs::create_dir_all(codex.join("sessions")).unwrap();
        fs::write(codex.join("config.toml"), "model = \"gpt\"\n").unwrap();
        fs::write(
            codex.join("auth.json"),
            r#"{"tokens":{"access_token":"a.b.c","account_id":"acct-own"}}"#,
        )
        .unwrap();
        Env { home }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn cswap_home(&self) -> PathBuf {
        self.home().join(".cswap")
    }

    fn log_path(&self) -> PathBuf {
        self.home().join("agents.log")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("CSWAP_HOME", self.cswap_home())
            .env("CSWAP_PROC", self.home().join("proc"))
            .env("CSWAP_OFFLINE", "1")
            .env("CSWAP_NO_UPDATE_CHECK", "1")
            .env("CSWAP_FAKE_LOG", self.log_path())
            .env("SHELL", "/bin/bash")
            .env_remove("ZDOTDIR")
            .env_remove("XDG_CONFIG_HOME")
            .env("NO_COLOR", "1")
            // Only the fake agents and the system's own tools.
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.home().join("bin").display()),
            )
            .stdin(std::process::Stdio::null());
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    /// Run and expect success. Returns stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "cswap {args:?} failed\nstdout: {}\nstderr: {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        text(&out.stdout)
    }

    /// Run and expect failure. Returns stderr.
    fn fails(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            !out.status.success(),
            "cswap {args:?} should have failed\nstdout: {}",
            text(&out.stdout)
        );
        text(&out.stderr)
    }

    /// What the agents logged since the last call, and forget it.
    fn log(&self) -> String {
        let text = fs::read_to_string(self.log_path()).unwrap_or_default();
        let _ = fs::remove_file(self.log_path());
        text
    }

    fn account_dir(&self, key: &str) -> PathBuf {
        self.cswap_home().join("accounts").join(key)
    }

    /// The accounts cswap holds. The agents' own logins are not among them.
    fn list(&self) -> Vec<Value> {
        serde_json::from_str(&self.ok(&["list", "--json"])).unwrap()
    }

    /// The agents' own logins.
    fn roots(&self) -> Vec<Value> {
        serde_json::from_str(&self.ok(&["root", "--json"])).unwrap()
    }

    fn account(&self, key: &str) -> Value {
        self.list()
            .into_iter()
            .chain(self.roots())
            .find(|a| a["key"] == key)
            .unwrap_or_else(|| panic!("no account {key}"))
    }

    /// Add a Claude Code account that signs in as `email`.
    fn add_claude(&self, name: &str, email: &str) {
        let out = self
            .cmd(&["add", "claude", name])
            .env("CSWAP_FAKE_EMAIL", email)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The arguments one logged run was started with.
fn args_of(log: &str) -> Vec<&str> {
    log.lines()
        .filter_map(|l| l.strip_prefix("arg: "))
        .collect()
}

/// Every path under `dir` with what it is, so two listings can be compared.
fn tree(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in fs::read_dir(&d).unwrap().flatten() {
            let path = e.path();
            let md = fs::symlink_metadata(&path).unwrap();
            let rel = path.strip_prefix(dir).unwrap().display().to_string();
            if md.is_dir() {
                out.push(format!("{rel}/"));
                todo.push(path);
            } else {
                out.push(format!(
                    "{rel} {}",
                    fs::read(&path).map(|b| b.len()).unwrap_or(0)
                ));
            }
        }
    }
    out.sort();
    out
}

// ------------------------------------------------------------------- wrapper

#[test]
fn an_agents_arguments_reach_it_untouched() {
    let env = Env::new();
    let out = env.ok(&[
        "claude",
        "-p",
        "two words",
        "--model",
        "opus",
        "--help",
        "-h",
        "--version",
        "--",
        "--cswapish",
        "",
    ]);
    assert!(out.contains("claude ran"), "{out}");
    let log = env.log();
    assert_eq!(
        args_of(&log),
        [
            "-p",
            "two words",
            "--model",
            "opus",
            "--help",
            "-h",
            "--version",
            "--",
            "--cswapish",
            ""
        ],
        "{log}"
    );
    // The agent's own login: nothing is redirected.
    assert!(log.contains("CLAUDE_CONFIG_DIR=\n"), "{log}");
    assert!(log.contains("CSWAP_ACCOUNT=claude-0\n"), "{log}");
}

#[test]
fn the_cswap_flag_picks_the_account_and_is_removed() {
    let env = Env::new();
    env.ok(&["add", "claude", "work"]);
    env.log();
    let dir = env.account_dir("claude-1").join("config_dir");

    for selector in [
        "--cswap=work",
        "--cswap=1",
        "--cswap=claude-1",
        "--cswap=new@example.com",
    ] {
        env.ok(&["claude", "--resume", selector, "fix the bug"]);
        let log = env.log();
        assert_eq!(
            args_of(&log),
            ["--resume", "fix the bug"],
            "{selector}: {log}"
        );
        assert!(
            log.contains(&format!("CLAUDE_CONFIG_DIR={}\n", dir.display())),
            "{selector}: {log}"
        );
        assert!(
            log.contains("CSWAP_ACCOUNT=claude-1\n"),
            "{selector}: {log}"
        );
    }

    // The agent's own login is asked for as `root`. `default` was its name
    // in earlier builds and still works.
    for selector in ["root", "default"] {
        env.ok(&["claude", &format!("--cswap={selector}")]);
        assert!(env.log().contains("CSWAP_ACCOUNT=claude-0\n"), "{selector}");
    }
}

#[test]
fn an_unknown_account_starts_nothing() {
    let env = Env::new();
    let err = env.fails(&["claude", "--cswap=nope", "-p", "hi"]);
    assert!(err.contains("nope"), "{err}");
    assert_eq!(env.log(), "", "the agent must not have been started");
}

#[test]
fn the_agents_exit_code_is_cswaps_exit_code() {
    let env = Env::new();
    let out = env
        .cmd(&["codex", "exec", "x"])
        .env("CSWAP_FAKE_EXIT", "7")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7));
}

#[test]
fn the_default_and_the_pin_variable_choose_the_account() {
    let env = Env::new();
    env.ok(&["add", "claude", "work"]);
    env.log();

    // Out of the box a bare launch stays on the agent's own login.
    env.ok(&["claude"]);
    assert!(env.log().contains("CSWAP_ACCOUNT=claude-0\n"));
    assert_eq!(env.account("claude-0")["starts_here"], true);
    assert!(env.ok(&["use", "claude"]).contains("root login"));

    // A pin for one shell wins over the default.
    let out = env
        .cmd(&["claude"])
        .env("CSWAP_CLAUDE", "work")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(env.log().contains("CSWAP_ACCOUNT=claude-1\n"));

    env.ok(&["use", "claude", "work"]);
    env.ok(&["claude"]);
    assert!(env.log().contains("CSWAP_ACCOUNT=claude-1\n"));
    assert_eq!(env.account("claude-1")["starts_here"], true);
    assert_eq!(env.account("claude-0")["starts_here"], false);
    assert!(env.ok(&["use", "claude"]).contains("work"));

    // The flag wins over both.
    let out = env
        .cmd(&["claude", "--cswap=0"])
        .env("CSWAP_CLAUDE", "work")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(env.log().contains("CSWAP_ACCOUNT=claude-0\n"));

    // Codex was never touched by any of this.
    env.ok(&["codex"]);
    assert!(env.log().contains("CSWAP_ACCOUNT=codex-0\n"));

    // And back to the agent's own login.
    env.ok(&["use", "claude", "default"]);
    env.ok(&["claude"]);
    assert!(env.log().contains("CSWAP_ACCOUNT=claude-0\n"));
    env.fails(&["use", "claude", "nobody"]);
}

#[test]
fn start_with_default_and_an_agent_needs_no_terminal() {
    let env = Env::new();
    env.ok(&["start", "--default", "codex", "--", "exec", "hello there"]);
    let log = env.log();
    assert!(log.contains("run: codex\n"), "{log}");
    assert_eq!(args_of(&log), ["exec", "hello there"], "{log}");
}

// ------------------------------------------------------------------ accounts

#[test]
fn add_shares_everything_but_the_login() {
    let env = Env::new();
    let native_before = tree(&env.home().join(".claude"));

    let out = env.ok(&["add", "claude", "work"]);
    assert!(
        out.contains("work is signed in as new@example.com"),
        "{out}"
    );
    let log = env.log();
    assert_eq!(args_of(&log), ["auth", "login"], "{log}");

    let dir = env.account_dir("claude-1").join("config_dir");
    // Shared: links into the agent's own dir.
    for name in ["settings.json", "CLAUDE.md", "projects", "skills"] {
        let md = fs::symlink_metadata(dir.join(name)).unwrap();
        assert!(md.file_type().is_symlink(), "{name} should be a link");
    }
    assert_eq!(
        fs::read_to_string(dir.join("settings.json")).unwrap(),
        r#"{"model":"opus"}"#
    );
    // Private: the login is a real file of its own.
    let creds = dir.join(".credentials.json");
    assert!(!fs::symlink_metadata(&creds)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(fs::read_to_string(&creds).unwrap().contains("tok-new"));
    assert_eq!(
        fs::metadata(env.cswap_home()).unwrap().permissions().mode() & 0o077,
        0,
        "~/.cswap must be private"
    );

    // Nothing was written into the agent's own dir, and its login is intact.
    assert_eq!(tree(&env.home().join(".claude")), native_before);

    let acc = env.account("claude-1");
    assert_eq!(acc["name"], "work");
    assert_eq!(acc["email"], "new@example.com");
    assert_eq!(acc["signed_in"], true);
    assert_eq!(env.account("claude-0")["email"], "own@example.com");
}

#[test]
fn the_agents_own_logins_are_roots_and_not_accounts() {
    let env = Env::new();
    assert!(env.list().is_empty());
    let keys: Vec<String> = env
        .roots()
        .iter()
        .map(|d| d["key"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(keys, ["claude-0", "codex-0"]);
    for d in env.roots() {
        assert_eq!(d["name"], "root");
        assert_eq!(d["same_login_as"], Value::Null);
    }

    // An account that signs in as the same person: the one pairing allowed.
    env.add_claude("mine", "own@example.com");
    let keys: Vec<String> = env
        .list()
        .iter()
        .map(|a| a["key"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(keys, ["claude-1"]);
    assert_eq!(env.account("claude-0")["same_login_as"], "mine");

    let text = env.ok(&["root"]);
    assert!(
        text.contains("own@example.com") && text.contains("same login as mine"),
        "{text}"
    );
    // The command's name in earlier builds still works.
    assert_eq!(env.ok(&["defaults"]), text);
    let list = env.ok(&["list"]);
    // One row: the account. The agent's own login is not listed with it.
    assert!(list.contains("mine"), "{list}");
    assert_eq!(list.matches("own@example.com").count(), 1, "{list}");
    // Nowhere is the old word for it.
    for cmd in [&["list"][..], &["root"], &["usage"], &["resets"], &["-h"]] {
        let out = env.ok(cmd).to_lowercase();
        assert!(!out.contains("native"), "cswap {cmd:?}\n{out}");
    }
}

#[test]
fn every_account_has_a_name() {
    let env = Env::new();
    // No name and nobody to ask: nothing is added and no sign-in runs.
    let err = env.fails(&["add", "claude"]);
    assert!(err.contains("name"), "{err}");
    for bad in ["3", "root", "default", "auto", "two words"] {
        let err = env.fails(&["add", "claude", bad]);
        assert!(err.contains("name"), "{bad}: {err}");
    }
    assert_eq!(env.log(), "", "no sign-in must have run");
    assert!(env.list().is_empty());

    env.add_claude("work", "a@example.com");
    env.add_claude("side", "b@example.com");
    // A name is one account's, within its agent.
    let err = env.fails(&["add", "claude", "work"]);
    assert!(err.contains("work"), "{err}");
    let err = env.fails(&["rename", "side", "work"]);
    assert!(err.contains("work"), "{err}");

    env.ok(&["rename", "side", "play"]);
    assert_eq!(env.account("claude-2")["name"], "play");
    env.log();
    env.ok(&["claude", "--cswap=play"]);
    assert!(env.log().contains("CSWAP_ACCOUNT=claude-2\n"));
    env.fails(&["claude", "--cswap=side"]);
    // The agent's own login is called root, and that cannot change.
    env.fails(&["rename", "claude-0", "mine"]);
}

#[test]
fn a_login_is_held_by_one_account_only() {
    let env = Env::new();
    env.add_claude("work", "a@example.com");
    env.log();

    // Signing in as the same person again adds nothing.
    let out = env
        .cmd(&["add", "claude", "again"])
        .env("CSWAP_FAKE_EMAIL", "a@example.com")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = text(&out.stderr);
    assert!(
        err.contains("work already holds the login a@example.com"),
        "{err}"
    );
    assert!(!env.account_dir("claude-2").exists());
    assert_eq!(env.list().len(), 1);
    // The name is free, for the login that is someone else's.
    env.add_claude("again", "b@example.com");
    assert_eq!(env.account("claude-2")["email"], "b@example.com");

    // Codex tells logins apart by the account behind them.
    env.ok(&["add", "codex", "team"]);
    let err = env.fails(&["add", "codex", "team2"]);
    assert!(err.contains("team already holds"), "{err}");
    let out = env
        .cmd(&["add", "codex", "team2"])
        .env("CSWAP_FAKE_ACCT", "acct-other")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
}

#[test]
fn a_file_the_agent_makes_later_is_shared_at_the_next_launch() {
    let env = Env::new();
    env.ok(&["add", "claude", "work"]);
    let dir = env.account_dir("claude-1").join("config_dir");
    fs::create_dir_all(env.home().join(".claude/plugins")).unwrap();
    assert!(fs::symlink_metadata(dir.join("plugins")).is_err());
    env.ok(&["claude", "--cswap=1"]);
    assert!(fs::symlink_metadata(dir.join("plugins"))
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn a_sign_in_that_fails_adds_nothing() {
    let env = Env::new();
    let out = env
        .cmd(&["add", "codex", "team"])
        .env("CSWAP_FAKE_FAIL", "1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("nothing was added"),
        "{}",
        text(&out.stderr)
    );
    assert!(!env.account_dir("codex-1").exists());
    assert!(env.list().is_empty());

    // The name is free again.
    env.ok(&["add", "codex", "team"]);
    assert_eq!(env.account("codex-1")["name"], "team");
}

#[test]
fn aliases_are_extra_names_that_can_be_set_shown_and_cleared() {
    let env = Env::new();
    env.add_claude("work", "a@example.com");
    env.add_claude("side", "b@example.com");
    assert_eq!(env.account("claude-2")["name"], "side");
    assert_eq!(env.account("claude-2")["aliases"], serde_json::json!([]));

    // An alias cannot be another account's name or alias.
    let err = env.fails(&["alias", "claude-2", "work"]);
    assert!(err.contains("work"), "{err}");
    env.ok(&["alias", "side", "play", "s"]);
    let acc = env.account("claude-2");
    assert_eq!(acc["name"], "side", "an alias does not replace the name");
    assert_eq!(acc["aliases"], serde_json::json!(["play", "s"]));
    let err = env.fails(&["alias", "work", "play"]);
    assert!(err.contains("play"), "{err}");
    let shown = env.ok(&["alias", "side"]);
    assert!(shown.contains("play") && shown.contains('s'), "{shown}");

    env.log();
    for name in ["side", "play", "s"] {
        env.ok(&["claude", &format!("--cswap={name}")]);
        assert!(env.log().contains("CSWAP_ACCOUNT=claude-2\n"), "{name}");
    }
    env.ok(&["alias", "side", "--clear"]);
    assert_eq!(env.account("claude-2")["aliases"], serde_json::json!([]));
    env.fails(&["claude", "--cswap=play"]);
    env.ok(&["claude", "--cswap=side"]);
}

#[test]
fn remove_deletes_the_account_and_leaves_the_agents_own_login() {
    let env = Env::new();
    env.ok(&["add", "claude", "work"]);
    env.ok(&["use", "claude", "work"]);

    // Without --yes and without a terminal, nothing is removed.
    env.fails(&["remove", "work"]);
    assert!(env.account_dir("claude-1").exists());

    env.ok(&["remove", "work", "--yes"]);
    assert!(!env.account_dir("claude-1").exists());
    assert!(env.home().join(".claude/.credentials.json").exists());
    // A bare launch fell back to the agent's own login.
    env.log();
    env.ok(&["claude"]);
    assert!(env.log().contains("CSWAP_ACCOUNT=claude-0\n"));

    let err = env.fails(&["remove", "claude-0", "--yes"]);
    assert!(err.contains("own login"), "{err}");
}

// ------------------------------------------------------------- look and help

#[test]
fn help_names_every_command_in_groups() {
    let env = Env::new();
    for flag in ["-h", "--help"] {
        let out = env.ok(&[flag]);
        for word in [
            "USAGE",
            "AGENTS",
            "LOOK",
            "ACCOUNTS",
            "UPKEEP",
            "SETUP",
            "EXAMPLES",
            "cswap start",
            "ps  ",
            "root  ",
            "add <agent> <name>",
            "rename <account> <name>",
            "use <agent> [account]",
            "update  ",
            "--cswap=",
            "--cswap-select",
        ] {
            assert!(out.contains(word), "{flag}: help lacks {word:?}\n{out}");
        }
        assert!(
            out.lines().all(|l| l.chars().count() <= 100),
            "a help line is too long\n{out}"
        );
    }
    assert!(env.ok(&["--version"]).contains(env!("CARGO_PKG_VERSION")));
    // A mistyped command is an error, not a silent dashboard.
    env.fails(&["nonsense"]);
}

#[test]
fn the_text_views_work_without_a_terminal() {
    let env = Env::new();
    env.ok(&["add", "claude", "work"]);
    let list = env.ok(&["list"]);
    assert!(
        list.contains("work") && list.contains("new@example.com"),
        "{list}"
    );
    assert!(!list.contains('\x1b'), "no colour codes when piped");
    for cmd in [
        &["usage"][..],
        &["usage", "work"],
        &["root"],
        &["resets"],
        &["suggest"],
        &["suggest", "claude"],
        &["ps"],
        &["sync", "--check"],
    ] {
        let out = env.run(cmd);
        assert!(out.status.success(), "cswap {cmd:?}: {}", text(&out.stderr));
    }
    assert_eq!(env.ok(&["ps", "--json"]).trim(), "[]");
    // The dashboard needs a terminal and says so.
    let err = env.fails(&[]);
    assert!(err.contains("terminal"), "{err}");
}

#[test]
fn shell_init_wraps_the_installed_agents() {
    let env = Env::new();
    let bash = env.ok(&["shell-init", "bash"]);
    assert!(
        bash.contains("claude() { command cswap claude \"$@\"; }"),
        "{bash}"
    );
    assert!(
        bash.contains("codex() { command cswap codex \"$@\"; }"),
        "{bash}"
    );
    assert!(
        !bash.contains("grok()"),
        "grok is not installed here\n{bash}"
    );
    let fish = env.ok(&["shell-init", "fish"]);
    assert!(fish.contains("command cswap claude $argv"), "{fish}");
    let ps = env.ok(&["shell-init", "powershell"]);
    assert!(
        ps.contains("function claude { cswap.exe claude @args }"),
        "{ps}"
    );
    env.fails(&["shell-init", "tcsh"]);

    // What bash prints really is a wrapper: sourced, `claude` goes through cswap.
    let script = format!("{bash}\nclaude --print 'a b'\n");
    let mut sh = Command::new("bash");
    sh.arg("-c").arg(&script);
    for (k, v) in env.cmd(&[]).get_envs() {
        if let Some(v) = v {
            sh.env(k, v);
        }
    }
    sh.env(
        "PATH",
        format!(
            "{}:{}:/usr/bin:/bin",
            Path::new(BIN).parent().unwrap().display(),
            env.home().join("bin").display()
        ),
    );
    env.log();
    let out = sh.output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let log = env.log();
    assert_eq!(args_of(&log), ["--print", "a b"], "{log}");
    assert!(log.contains("CSWAP_ACCOUNT=claude-0\n"), "{log}");

    // A shell with a list of its own wraps what the list holds and no more.
    // The dashboard writes the list, under Settings > Shell.
    fs::create_dir_all(env.cswap_home()).unwrap();
    fs::write(
        env.cswap_home().join("settings.json"),
        r#"{"shells":{"bash":["codex"]}}"#,
    )
    .unwrap();
    let bash = env.ok(&["shell-init", "bash"]);
    assert!(
        bash.contains("codex()") && !bash.contains("claude()"),
        "{bash}"
    );
    // Another shell has no list yet, so it still wraps them all.
    assert!(env.ok(&["shell-init", "zsh"]).contains("claude()"));
}

#[test]
fn doctor_reports_and_migrate_has_nothing_to_do_on_a_fresh_home() {
    let env = Env::new();
    let out = env.run(&["doctor"]);
    let all = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(all.contains("Claude Code"), "{all}");
    let out = env.ok(&["migrate"]);
    assert!(out.to_lowercase().contains("nothing"), "{out}");
}
