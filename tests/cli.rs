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
        let mut c = self.program(BIN);
        c.args(args);
        c
    }

    /// Any program, in this home's environment and with nothing of the real
    /// one: the tests may themselves be run by an agent that cswap started.
    fn program(&self, bin: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = Command::new(bin);
        c.env_clear()
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
            "FOR AGENTS",
            "cswap agent-info",
            "USAGE",
            "AGENTS",
            "LOOK",
            "ACCOUNTS",
            "UPKEEP",
            "SETUP",
            "EXAMPLES",
            "cswap start",
            "cswap dashboard [page]",
            "ps  ",
            "root  ",
            "add <agent> <name>",
            "rename <account> <name>",
            "use <agent> [account]",
            "update  ",
            "shims [on|off]",
            "uninstall  ",
            "herdr switch",
            "herdr restart <t>",
            "herdr install",
            "--cswap=",
            "--cswap  ",
        ] {
            assert!(out.contains(word), "{flag}: help lacks {word:?}\n{out}");
        }
        assert!(
            out.lines().all(|l| l.chars().count() <= 100),
            "a help line is too long\n{out}"
        );
        // The guide for agents is named before anything else.
        assert!(
            out.find("agent-info").unwrap() < out.find("USAGE").unwrap(),
            "{flag}: agent-info is not at the top\n{out}"
        );
    }
    assert!(env.ok(&["--version"]).contains(env!("CARGO_PKG_VERSION")));
    // A mistyped command is an error, not a silent dashboard.
    env.fails(&["nonsense"]);
}

/// A stand-in for herdr's `hc`. The terminal `t7` has the process named in
/// `hc/pid` in front; Ctrl+C ends it, and a send puts the next agent there.
/// With `hc/direct` the agent is the terminal's own program until it dies,
/// and a shell after that, as herdr does for a kept terminal. `hc/ls` is what
/// `hc ls` lists: the terminals under `t7`. With `hc/swallow` the shell has
/// text at its prompt that swallows the next line sent, until a Ctrl+C
/// clears it.
const FAKE_HC: &str = r#"#!/bin/sh
dir="$HOME/hc"
old=$(cat "$dir/pid")
case "$1" in
  server) printf '{"holder":{"instance":"%s"}}\n' "$(cat "$dir/core" 2>/dev/null)" ;;
  run) echo "$*" >> "$dir/log"; echo '{"address":"t9"}' ;;
  get)
    if [ -f "$dir/gone" ]; then echo "no terminal" >&2; exit 3; fi
    if kill -0 "$old" 2>/dev/null; then front=$old
    elif [ -f "$dir/sent" ]; then front=$(cat "$dir/next")
    else front=1; fi
    if [ -f "$dir/direct" ] && [ "$front" = "$old" ]; then argv=claude; else argv=/bin/zsh; fi
    if [ -f "$dir/keep" ]; then keep=true; else keep=false; fi
    if [ "$front" = 1 ]; then name=zsh; else name=claude; fi
    printf '{"address":"t7","state":"live","keep":%s,"argv":["%s"],' "$keep" "$argv"
    printf '"status":{"state":"%s"},' "$(cat "$dir/status")"
    printf '"foreground":{"pid":%s,"name":"%s"}}\n' "$front" "$name"
    ;;
  ls)
    if [ -f "$dir/ls" ]; then cat "$dir/ls"; else echo '{"terminals":[]}'; fi
    ;;
  keys)
    echo "$*" >> "$dir/log"
    if kill "$old" 2>/dev/null; then :
    elif [ -f "$dir/swallow" ]; then
      n=$(($(cat "$dir/swallow") - 1))
      if [ "$n" -le 0 ]; then rm "$dir/swallow"; else echo "$n" > "$dir/swallow"; fi
    fi
    ;;
  keep) echo "$*" >> "$dir/log"; : > "$dir/keep" ;;
  send)
    echo "$*" >> "$dir/log"
    if [ ! -f "$dir/swallow" ]; then
      mv "$dir/next-proc" "$HOME/proc/$(cat "$dir/next")"
      : > "$dir/sent"
    fi
    ;;
esac
exit 0
"#;

/// A process that lives until it is killed and belongs to nobody, so that it
/// is gone the moment it ends. Returns its pid.
fn orphan() -> u32 {
    let out = Command::new("sh")
        .args(["-c", "sleep 300 >/dev/null 2>&1 & echo $!"])
        .output()
        .unwrap();
    text(&out.stdout).trim().parse().unwrap()
}

const SESSION: &str = "32efd4c2-3347-4240-a60a-61b639bc1ece";

/// A home with a `work` account beside the root login, herdr's stand-in on
/// the PATH, and Claude Code running in terminal t7 on the root login with
/// the command line `cmdline`, in a conversation that has a transcript.
/// Returns the pid that will be the agent once it runs on `work`.
fn herdr_terminal(env: &Env, cmdline: &str) -> u32 {
    env.add_claude("work", "work@example.com");
    let h = env.home();
    let hc = h.join("bin/hc");
    fs::write(&hc, FAKE_HC).unwrap();
    fs::set_permissions(&hc, fs::Permissions::from_mode(0o755)).unwrap();

    let (old, next) = (orphan(), orphan());
    let proc_of = |dir: PathBuf, environ: &str, cmdline: &str| {
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("comm"), "claude\n").unwrap();
        fs::write(dir.join("environ"), environ).unwrap();
        fs::write(dir.join("cmdline"), cmdline).unwrap();
    };
    proc_of(
        h.join("proc").join(old.to_string()),
        "HC_TERMINAL_ID=t7\0",
        cmdline,
    );
    fs::create_dir_all(h.join("hc")).unwrap();
    fs::write(h.join("hc/pid"), old.to_string()).unwrap();
    fs::write(h.join("hc/next"), next.to_string()).unwrap();
    fs::write(h.join("hc/status"), "idle").unwrap();
    fs::create_dir_all(h.join(".claude/sessions")).unwrap();
    fs::write(
        h.join(format!(".claude/sessions/{old}.json")),
        format!(r#"{{"pid":{old},"sessionId":"{SESSION}"}}"#),
    )
    .unwrap();
    fs::write(
        h.join(format!(".claude/projects/proj-a/{SESSION}.jsonl")),
        "{}\n",
    )
    .unwrap();
    // What the agent will look like once it runs on `work`.
    let work_dir = env.account_dir("claude-1").join("config_dir");
    proc_of(
        h.join("hc/next-proc"),
        &format!("CLAUDE_CONFIG_DIR={}\0", work_dir.display()),
        "claude\0",
    );
    next
}

#[test]
fn an_agent_that_is_its_terminals_own_program_is_switched_without_ending_the_terminal() {
    let env = Env::new();
    let next = herdr_terminal(&env, "claude\0--model\0opus\0--continue\0the task\0");
    let h = env.home();
    // No shell started this agent, and herdr is not keeping the terminal.
    fs::write(h.join("hc/direct"), "").unwrap();

    let done: Value =
        serde_json::from_str(&env.ok(&["herdr", "switch", "t7", "work", "--json"])).unwrap();
    assert_eq!(done["resumed"], true);
    assert_eq!(done["started"], true);
    // Keeping is turned on first, so herdr puts a shell where the agent was.
    // No Ctrl+C: a clean exit would end the terminal. The model comes along.
    let log = fs::read_to_string(h.join("hc/log")).unwrap();
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        [
            "keep t7",
            &format!("send t7 cswap claude --cswap=work --model opus --resume {SESSION}"),
        ]
    );
    let _ = Command::new("kill").arg(next.to_string()).status();
}

#[test]
fn a_herdr_terminal_moves_to_another_account_and_keeps_its_conversation() {
    let env = Env::new();
    let next = herdr_terminal(&env, "claude\0");
    let h = env.home();
    fs::write(h.join("hc/status"), "working").unwrap();

    let all: Value = serde_json::from_str(&env.ok(&["herdr", "ls", "--json"])).unwrap();
    assert_eq!(all.as_array().unwrap().len(), 1);
    assert_eq!(all[0]["terminal"], "t7");
    assert_eq!(all[0]["name"], "root");
    let info: Value = serde_json::from_str(&env.ok(&["herdr", "info", "t7", "--json"])).unwrap();
    assert_eq!(info["account"], "claude-0");
    assert_eq!(info["session"], SESSION);
    let current: Vec<&Value> = info["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["current"] == true)
        .collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0]["name"], "root");

    // Not in the middle of a turn, and not onto the account it is on.
    let err = env.fails(&["herdr", "switch", "t7", "work"]);
    assert!(err.contains("middle of a turn"), "{err}");
    fs::write(h.join("hc/status"), "idle").unwrap();
    let err = env.fails(&["herdr", "switch", "t7", "root"]);
    assert!(err.contains("already runs on root"), "{err}");
    assert!(!h.join("hc/log").exists(), "a refusal touched the terminal");

    let done: Value =
        serde_json::from_str(&env.ok(&["herdr", "switch", "t7", "work", "--json"])).unwrap();
    assert_eq!(done["from"], "root");
    assert_eq!(done["to"], "work");
    assert_eq!(done["resumed"], true);
    assert_eq!(done["started"], true);
    let log = fs::read_to_string(h.join("hc/log")).unwrap();
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        [
            "keys t7 ctrl+c",
            // The prompt is cleared before the launch is typed.
            "keys t7 ctrl+c",
            &format!("send t7 cswap claude --cswap=work --resume {SESSION}"),
        ]
    );
    let info: Value = serde_json::from_str(&env.ok(&["herdr", "info", "t7", "--json"])).unwrap();
    assert_eq!(info["account"], "claude-1");

    let _ = Command::new("kill").arg(next.to_string()).status();
}

/// A restart is a switch onto the account the agent is on: it comes back in
/// the same terminal and conversation, as the program that is on disk now.
#[test]
fn a_herdr_terminal_restarts_its_agent_on_the_same_account() {
    let env = Env::new();
    let next = herdr_terminal(&env, "claude\0--model\0opus\0");
    let h = env.home();
    // What the agent looks like once it runs again: on its root login.
    fs::write(h.join("hc/next-proc/environ"), "HC_TERMINAL_ID=t7\0").unwrap();

    // Not in the middle of a turn.
    fs::write(h.join("hc/status"), "working").unwrap();
    let err = env.fails(&["herdr", "restart", "t7"]);
    assert!(err.contains("middle of a turn"), "{err}");
    assert!(!h.join("hc/log").exists(), "a refusal touched the terminal");
    fs::write(h.join("hc/status"), "idle").unwrap();
    // A switch onto the same account says how to restart instead.
    let err = env.fails(&["herdr", "switch", "t7", "root"]);
    assert!(err.contains("cswap herdr restart t7"), "{err}");

    let done: Value = serde_json::from_str(&env.ok(&["herdr", "restart", "t7", "--json"])).unwrap();
    assert_eq!(done["account"], "root");
    assert_eq!(done["resumed"], true);
    assert_eq!(done["started"], true);
    let log = fs::read_to_string(h.join("hc/log")).unwrap();
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        [
            "keys t7 ctrl+c",
            "keys t7 ctrl+c",
            &format!("send t7 cswap claude --cswap=root --model opus --resume {SESSION}"),
        ]
    );
    let info: Value = serde_json::from_str(&env.ok(&["herdr", "info", "t7", "--json"])).unwrap();
    assert_eq!(info["account"], "claude-0");
    assert_eq!(info["pid"], next);

    let _ = Command::new("kill").arg(next.to_string()).status();
}

/// Herdr was stopped: the terminal is back as a shell and the agent is gone.
#[test]
fn a_saved_agent_is_started_again_in_its_terminal_after_a_stop() {
    let env = Env::new();
    let next = herdr_terminal(&env, "claude\0--model\0opus\0");
    let h = env.home();
    fs::write(h.join("hc/next-proc/environ"), "HC_TERMINAL_ID=t7\0").unwrap();
    fs::write(h.join("hc/core"), "one").unwrap();

    let saved: Value = serde_json::from_str(&env.ok(&["herdr", "save", "--json"])).unwrap();
    let e = &saved["terminals"][0];
    assert_eq!(saved["terminals"].as_array().unwrap().len(), 1);
    assert_eq!(e["terminal"], "t7");
    assert_eq!(e["agent"], "claude");
    assert_eq!(e["account"], "claude-0");
    assert_eq!(e["session"], SESSION);
    assert_eq!(e["options"], serde_json::json!(["--model", "opus"]));
    // The layout asks every minute; cswap writes only when a save is due.
    let soon: Value =
        serde_json::from_str(&env.ok(&["herdr", "save", "--auto", "--json"])).unwrap();
    assert_eq!(soon["wrote"], "not_due");
    assert_eq!(soon["every_minutes"], 10);
    assert_eq!(soon["agents"], 1);

    // The stop: the agent ends, and herdr's terminals are another set.
    let old = fs::read_to_string(h.join("hc/pid")).unwrap();
    let _ = Command::new("kill").arg(old.trim()).status();
    while Command::new("kill")
        .args(["-0", old.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    fs::write(h.join("hc/core"), "two").unwrap();

    // The layout's timer does not write an empty list over the one to keep.
    let auto: Value =
        serde_json::from_str(&env.ok(&["herdr", "save", "--auto", "--json"])).unwrap();
    assert_eq!(auto["wrote"], "kept");
    assert_eq!(auto["pending"], true);
    assert_eq!(auto["terminals"][0]["session"], SESSION);

    let launch = format!("cswap claude --cswap=root --model opus --resume {SESSION}");
    let line = match e["cwd"].as_str() {
        Some(cwd) => format!("cd {cwd} && {launch}"),
        None => launch,
    };
    let plan: Value =
        serde_json::from_str(&env.ok(&["herdr", "restore", "--dry-run", "--json"])).unwrap();
    assert_eq!(plan["terminals"][0]["did"], "typed");
    assert_eq!(plan["terminals"][0]["line"], line.as_str());
    assert_eq!(plan["terminals"][0]["manual"], line.as_str());
    assert!(!h.join("hc/log").exists(), "a dry run touched the terminal");

    let done: Value = serde_json::from_str(&env.ok(&["herdr", "restore", "--json"])).unwrap();
    assert_eq!(done["ok"], true);
    assert_eq!(done["terminals"][0]["did"], "typed");
    assert_eq!(done["terminals"][0]["started"], true);
    let log = fs::read_to_string(h.join("hc/log")).unwrap();
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        ["keys t7 ctrl+c", &format!("send t7 {line}")]
    );
    // What runs now is saved, and the list that was used is kept beside it.
    assert!(env.cswap_home().join("herdr-saved.prev.json").exists());
    // A second restore finds the agent running and leaves it alone.
    let again: Value =
        serde_json::from_str(&env.ok(&["herdr", "restore", "--previous", "--json"])).unwrap();
    assert_eq!(again["terminals"][0]["did"], "skipped");

    let _ = Command::new("kill").arg(next.to_string()).status();
}

/// A worker that herdr did not keep is gone after a stop.
#[test]
fn a_saved_agent_whose_terminal_is_gone_gets_a_new_one() {
    let env = Env::new();
    let next = herdr_terminal(&env, "claude\0");
    let h = env.home();
    let entry = |terminal: &str, name: &str, conversation: &str| {
        serde_json::json!({
            "terminal": terminal, "name": name, "agent": "claude",
            "account": "claude-0", "account_name": "root",
            "conversation": conversation, "session": SESSION,
            "resume": ["--resume", SESSION], "options": [],
            "cwd": h, "parent": "t1", "keep": false, "outlive": true, "notify": false,
        })
    };
    let saved = serde_json::json!({
        "saved_at": 0, "core": "one",
        "terminals": [entry("t7", "build", "open"), entry("t8", "lost", "unknown")],
    });
    fs::create_dir_all(env.cswap_home()).unwrap();
    fs::write(env.cswap_home().join("herdr-saved.json"), saved.to_string()).unwrap();
    fs::write(h.join("hc/gone"), "").unwrap();

    let err = env.fails(&["herdr", "restore", "t99"]);
    assert!(err.contains("t99 is not in the save"), "{err}");

    let done: Value =
        serde_json::from_str(&env.ok(&["herdr", "restore", "build", "lost", "--json"])).unwrap();
    assert_eq!(done["terminals"][0]["did"], "run");
    assert_eq!(done["terminals"][0]["now"], "t9");
    // Never guessed: a conversation that was not known is not resumed.
    assert_eq!(done["terminals"][1]["did"], "skipped");
    // Its parent is gone too, so it gets none, not the caller as one.
    let log = fs::read_to_string(h.join("hc/log")).unwrap();
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        [&format!(
            "run --bg --json --name build --cwd {} --no-parent --no-notify cswap claude --cswap=root --resume {SESSION}",
            h.display()
        )]
    );

    let _ = Command::new("kill").arg(next.to_string()).status();
}

/// A sub-agent reports to its parent by having herdr type into the parent's
/// terminal. While the parent is stopped that text lands in a bare shell.
#[test]
fn an_agent_whose_sub_agents_are_working_is_left_alone_unless_forced() {
    let env = Env::new();
    let next = herdr_terminal(&env, "claude\0");
    let h = env.home();
    fs::write(h.join("hc/next-proc/environ"), "HC_TERMINAL_ID=t7\0").unwrap();
    // Under t7: one done, one working, and one working under the done one.
    fs::write(
        h.join("hc/ls"),
        r#"{"terminals":[
            {"address":"t7","state":"live","status":"idle"},
            {"address":"t8","parent":"t7","state":"live","status":"done"},
            {"address":"t9","parent":"t7","state":"live","status":"working"},
            {"address":"t10","parent":"t8","state":"live","status":"blocked"},
            {"address":"t11","state":"live","status":"working"}]}"#,
    )
    .unwrap();

    let info: Value = serde_json::from_str(&env.ok(&["herdr", "info", "t7", "--json"])).unwrap();
    assert_eq!(info["children_working"], serde_json::json!(["t10", "t9"]));
    assert_eq!(info["why_not"], Value::Null);
    for args in [
        vec!["herdr", "restart", "t7"],
        vec!["herdr", "switch", "t7", "work"],
    ] {
        let err = env.fails(&args);
        assert!(err.contains("2 sub-agents still working: t10, t9"), "{err}");
        assert!(err.contains("--force"), "{err}");
    }
    assert!(!h.join("hc/log").exists(), "a refusal touched the terminal");

    let done: Value =
        serde_json::from_str(&env.ok(&["herdr", "restart", "t7", "--force", "--json"])).unwrap();
    assert_eq!(done["started"], true);
    assert_eq!(done["resumed"], true);
    let _ = Command::new("kill").arg(next.to_string()).status();
}

/// Text that reached the shell before the launch, such as a report with an
/// apostrophe in it, leaves the shell waiting for the end of a quote, and a
/// line typed there never runs.
#[test]
fn a_launch_that_the_shell_swallowed_is_typed_again() {
    let env = Env::new();
    let next = herdr_terminal(&env, "claude\0");
    let h = env.home();
    fs::write(h.join("hc/next-proc/environ"), "HC_TERMINAL_ID=t7\0").unwrap();
    // The text arrives after the prompt was cleared: it takes the Ctrl+C
    // that stops the agent, the one that clears, and one more.
    fs::write(h.join("hc/swallow"), "2").unwrap();

    let done: Value = serde_json::from_str(&env.ok(&["herdr", "restart", "t7", "--json"])).unwrap();
    assert_eq!(done["started"], true, "{done}");
    let line = format!("send t7 cswap claude --cswap=root --resume {SESSION}");
    let log = fs::read_to_string(h.join("hc/log")).unwrap();
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        [
            "keys t7 ctrl+c",
            "keys t7 ctrl+c",
            // Swallowed: the shell is still in front, so cswap clears the
            // prompt and types the line again.
            &line,
            "keys t7 ctrl+c",
            &line,
        ]
    );
    let _ = Command::new("kill").arg(next.to_string()).status();
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
    // It opens on a page when one is named, and a wrong name is said first.
    let err = env.fails(&["dashboard", "Usage"]);
    assert!(err.contains("terminal"), "{err}");
    let err = env.fails(&["dashboard", "nonsense"]);
    assert!(
        err.contains("'nonsense' is not a page") && err.contains("usage, resets"),
        "{err}"
    );
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
    let mut sh = env.program("bash");
    sh.arg("-c").arg(&script);
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

/// `cswap herdr install` puts the layout's files into herdr's folder from a
/// checkout of this repository, and `cswap herdr status` says where it stands.
#[test]
fn the_herdr_layout_is_installed_from_the_repository() {
    let env = Env::new();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("integrations");
    let herdr = env.home().join("herdr-home");
    let run = |args: &[&str]| {
        env.cmd(args)
            .env("CSWAP_INTEGRATIONS", &source)
            .env("HERDR_APP_HOME", &herdr)
            .output()
            .unwrap()
    };

    // Without herdr there is nothing to install into.
    let out = run(&["herdr", "install"]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("Start herdr once"),
        "{}",
        text(&out.stderr)
    );
    assert!(text(&run(&["herdr", "status"]).stdout).contains("not on this machine"));

    fs::create_dir_all(&herdr).unwrap();
    let out = run(&["herdr", "status"]);
    assert!(
        !out.status.success(),
        "status is false until it is installed"
    );
    assert!(text(&out.stdout).contains("not installed"));

    let out = run(&["herdr", "install"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("Installed the cswap layout"),
        "{}",
        text(&out.stdout)
    );
    for file in [
        "layouts/cswap.json",
        "layouts/cswap.svg",
        "mods/cswap-agents/main.js",
        "mods/cswap-tabs/mod.json",
    ] {
        assert!(herdr.join(file).is_file(), "{file} is missing");
    }
    // Nothing but the layout's own files went in.
    let tops: Vec<String> = fs::read_dir(&herdr)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        tops.iter().all(|t| t == "layouts" || t == "mods"),
        "{tops:?}"
    );

    let out = run(&["herdr", "status", "--json"]);
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["installed"], true);
    assert_eq!(v["installed_by"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        (v["missing"].as_u64(), v["changed"].as_u64()),
        (Some(0), Some(0))
    );

    // A second install changes nothing; an edited file is kept beside the new one.
    assert!(text(&run(&["herdr", "install"]).stdout).contains("all current"));
    fs::write(herdr.join("layouts/cswap.json"), "{}").unwrap();
    let out = run(&["herdr", "install", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["written"], 1);
    assert_eq!(v["set_aside"][0], "layouts/cswap.json.mine");
    assert_eq!(
        fs::read_to_string(herdr.join("layouts/cswap.json.mine")).unwrap(),
        "{}"
    );

    // Offline and with no checkout to read, it says so and changes nothing.
    let out = env
        .cmd(&["herdr", "install"])
        .env("HERDR_APP_HOME", &herdr)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("offline"),
        "{}",
        text(&out.stderr)
    );
}

/// `cswap agent-info` is a guide an agent can act on: every command it names
/// is one this binary has.
#[test]
fn the_guide_for_agents_names_only_real_commands() {
    let env = Env::new();
    let guide = env.ok(&["agent-info"]);
    for word in [
        "a guide for agents",
        env!("CARGO_PKG_VERSION"),
        "$CSWAP_ACCOUNT",
        "cswap claude --cswap=work",
        "hc run --bg --name w1 cswap claude",
        "Never switch or restart your own terminal",
        "Leave these to the user",
    ] {
        assert!(guide.contains(word), "the guide lacks {word:?}\n{guide}");
    }
    // The commands it tells an agent to run all exist and take those flags.
    for args in [
        vec!["ps", "--json"],
        vec!["list", "--json"],
        vec!["suggest"],
        vec!["usage"],
        vec!["herdr", "ls", "-h"],
        vec!["herdr", "info", "-h"],
        vec!["herdr", "switch", "-h"],
        vec!["herdr", "restart", "-h"],
        vec!["refresh", "-h"],
    ] {
        env.ok(&args);
    }
    assert!(env.ok(&["usage", "-h"]).contains("--refresh"));
}

/// herdr starts a new terminal in the login environment, so a sub-agent does
/// not get its parent's account. It starts on the default of its own agent,
/// which is one rule for every pair of agents: a Codex under a Claude has no
/// account in common with it anyway.
/// `codex update` finds its installation under its own home, so it fails on
/// an account's home with "Could not detect the Codex installation method".
#[test]
fn a_command_about_the_program_itself_runs_on_its_own_home() {
    let env = Env::new();
    env.add_claude("work", "work@example.com");
    env.ok(&["use", "claude", "work"]);
    env.log();

    // A launch goes to the default account.
    env.ok(&["claude", "the task"]);
    assert!(env.log().contains("CSWAP_ACCOUNT=claude-1"));
    // An update does not, even when an account is named.
    for args in [
        &["claude", "update"][..],
        &["claude", "--cswap=work", "update"],
    ] {
        env.ok(args);
        let log = env.log();
        assert!(log.contains("arg: update"), "{log}");
        assert!(log.contains("CLAUDE_CONFIG_DIR=\n"), "{log}");
    }
    // Only as the first argument: a task that says "update" is a task.
    env.ok(&["claude", "-p", "update"]);
    assert!(env.log().contains("CSWAP_ACCOUNT=claude-1"));
}

#[test]
fn a_sub_agent_in_herdr_starts_on_its_agents_default() {
    let env = Env::new();
    env.add_claude("work", "work@example.com");
    env.add_claude("side", "side@example.com");
    env.ok(&["use", "claude", "side"]);
    let h = env.home();
    // herdr is never asked about the terminal above: this one would fail.
    let hc = h.join("bin/hc");
    fs::write(&hc, "#!/bin/sh\necho asked >> \"$HOME/hc-asked\"\nexit 1\n").unwrap();
    fs::set_permissions(&hc, fs::Permissions::from_mode(0o755)).unwrap();
    env.log();

    // In a terminal an agent on `work` started: the default, `side`.
    let out = env
        .cmd(&["claude", "the task"])
        .env("HC_TERMINAL_ID", "t9")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let log = env.log();
    assert!(log.contains("arg: the task"), "{log}");
    assert!(log.contains("CSWAP_ACCOUNT=claude-2"), "{log}");
    assert!(!h.join("hc-asked").exists());

    // Another kind of agent there starts on its own default.
    let out = env
        .cmd(&["codex"])
        .env("HC_TERMINAL_ID", "t9")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(env.log().contains("CSWAP_ACCOUNT=codex-0"));

    // An account named on the command line still wins.
    let out = env
        .cmd(&["claude", "--cswap=work"])
        .env("HC_TERMINAL_ID", "t9")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(env.log().contains("CSWAP_ACCOUNT=claude-1"));
}

// --------------------------------------------------------------- PATH shims

impl Env {
    fn shims(&self) -> PathBuf {
        self.cswap_home().join("shims")
    }

    /// The `PATH` of a program that sees the shims first, as after a login.
    fn shim_path(&self) -> String {
        format!(
            "{}:{}:/usr/bin:/bin",
            self.shims().display(),
            self.home().join("bin").display()
        )
    }

    /// Run `program` the way another program would: by its bare name, found
    /// on a `PATH` that has the shims first.
    fn bare(&self, program: &str, args: &[&str]) -> Output {
        self.program("sh")
            .arg("-c")
            .arg(format!("exec {program} \"$@\""))
            .arg("sh")
            .args(args)
            .env("PATH", self.shim_path())
            .output()
            .unwrap()
    }
}

#[test]
fn shims_are_off_until_asked_for_and_leave_nothing_behind() {
    let env = Env::new();
    assert!(env.ok(&["shims"]).contains("PATH shims are off"));
    assert!(!env.shims().exists());
    let profile = env.home().join(".profile");

    // No login file is there, so cswap makes one, and takes it away again.
    let out = env.ok(&["shims", "on"]);
    assert!(out.contains("on for claude, codex"), "{out}");
    assert!(out.contains("~/.profile"), "{out}");
    assert!(out.contains("hc server restart --fresh"), "{out}");
    for name in ["claude", "codex"] {
        let link = env.shims().join(name);
        assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(
            fs::canonicalize(&link).unwrap(),
            fs::canonicalize(BIN).unwrap()
        );
    }
    // Grok is not installed here, so it gets no shim.
    assert!(!env.shims().join("grok").exists());
    let text = fs::read_to_string(&profile).unwrap();
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(text.contains("\"$HOME/.cswap/shims:$PATH\""), "{text}");
    // Asking again adds nothing.
    env.ok(&["shims", "on"]);
    assert_eq!(fs::read_to_string(&profile).unwrap(), text);

    let out = env.ok(&["shims", "off"]);
    assert!(out.contains("~/.profile"), "{out}");
    assert!(!profile.exists(), "a file cswap made is still there");
    assert!(!env.shims().exists());
    assert!(env.ok(&["shims"]).contains("PATH shims are off"));

    // A login file of the user's gets the line at its end and comes back
    // byte for byte. A link stays a link.
    let dotfiles = env.home().join("dotfiles");
    fs::create_dir_all(&dotfiles).unwrap();
    let mine = "export EDITOR=vi\n# no newline at the end";
    fs::write(dotfiles.join("profile"), mine).unwrap();
    std::os::unix::fs::symlink(dotfiles.join("profile"), &profile).unwrap();
    fs::write(env.home().join(".bash_profile"), ". ~/.profile\n").unwrap();

    let out = env.ok(&["shims", "on", "claude"]);
    assert!(
        out.contains("~/.profile") && out.contains("~/.bash_profile"),
        "{out}"
    );
    assert!(env.shims().join("claude").exists());
    assert!(!env.shims().join("codex").exists());
    let text = fs::read_to_string(&profile).unwrap();
    assert!(text.starts_with(mine) && text.contains("cswap shims off"));

    // Naming another agent later replaces the list.
    env.ok(&["shims", "on", "codex"]);
    assert!(!env.shims().join("claude").exists());
    assert!(env.shims().join("codex").exists());

    env.ok(&["shims", "off"]);
    assert!(fs::symlink_metadata(&profile).unwrap().is_symlink());
    assert_eq!(fs::read_to_string(&profile).unwrap(), format!("{mine}\n"));
    assert_eq!(
        fs::read_to_string(env.home().join(".bash_profile")).unwrap(),
        ". ~/.profile\n"
    );
    assert!(!env.shims().exists());

    assert!(env.fails(&["shims", "on", "nonsense"]).contains("nonsense"));
    assert!(!env.shims().exists());
}

#[test]
fn an_agent_another_program_starts_goes_through_its_shim() {
    let env = Env::new();
    env.add_claude("work", "work@example.com");
    env.ok(&["use", "claude", "work"]);
    env.ok(&["shims", "on"]);
    env.log();

    // A bare `claude`, found on PATH by a program that knows nothing of cswap.
    let out = env.bare("claude", &["--print", "a b"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let log = env.log();
    assert_eq!(log.matches("run: claude").count(), 1, "{log}");
    assert_eq!(args_of(&log), ["--print", "a b"], "{log}");
    assert!(log.contains("CSWAP_ACCOUNT=claude-1\n"), "{log}");

    // cswap's own flag works there too, and so does every other agent.
    let out = env.bare("claude", &["--cswap=0", "-p"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let log = env.log();
    assert_eq!(args_of(&log), ["-p"], "{log}");
    assert!(log.contains("CSWAP_ACCOUNT=claude-0\n"), "{log}");
    let out = env.bare("codex", &["exec", "x"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let log = env.log();
    assert!(log.contains("run: codex") && log.contains("CSWAP_ACCOUNT=codex-0\n"));

    // The agent's exit code is still the caller's.
    let out = env
        .program(env.shims().join("claude"))
        .env("PATH", env.shim_path())
        .env("CSWAP_FAKE_EXIT", "7")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7));

    // Where they stand, from an environment that has the folder first, and
    // from one that does not.
    let out = env
        .cmd(&["shims"])
        .env("PATH", env.shim_path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stdout));
    assert!(
        text(&out.stdout).contains("claude runs through cswap from here"),
        "{}",
        text(&out.stdout)
    );
    let out = env.run(&["shims"]);
    assert!(!out.status.success());
    let said = text(&out.stdout);
    assert!(said.contains("comes first on PATH here"), "{said}");
    assert!(said.contains("hc server restart --fresh"), "{said}");
}

#[test]
fn a_wrapper_that_runs_the_shim_again_is_stopped() {
    let env = Env::new();
    env.ok(&["shims", "on", "claude"]);
    // Someone's own `claude` script that runs `claude` from PATH. With the
    // shim first on PATH that is cswap again, and so on for ever.
    fs::write(
        env.home().join("bin/claude"),
        "#!/bin/sh\necho again >> \"$CSWAP_FAKE_LOG\"\nexec claude \"$@\"\n",
    )
    .unwrap();
    let out = env.bare("claude", &["-p"]);
    assert!(!out.status.success());
    let err = text(&out.stderr);
    assert!(err.contains("keeps starting cswap again"), "{err}");
    assert!(err.contains("cswap shims off"), "{err}");
    assert_eq!(env.log().matches("again").count(), 4);
}

#[test]
fn shell_init_puts_the_shims_first_again() {
    let env = Env::new();
    assert!(!env.ok(&["shell-init", "bash"]).contains("shims"));
    env.ok(&["shims", "on"]);
    let bash = env.ok(&["shell-init", "bash"]);
    assert!(bash.contains("claude() { command cswap claude"), "{bash}");
    assert!(env.ok(&["shell-init", "fish"]).contains("set -gx PATH"));

    // A shim covers every shell: a shell whose own list leaves the agent out
    // wraps it all the same, and goes back to its list when the shim goes.
    let settings = env.cswap_home().join("settings.json");
    let mut json: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    json["shells"] = serde_json::json!({ "zsh": ["codex"] });
    fs::write(&settings, json.to_string()).unwrap();
    env.ok(&["shims", "on", "claude"]);
    let zsh = env.ok(&["shell-init", "zsh"]);
    assert!(zsh.contains("claude()") && zsh.contains("codex()"), "{zsh}");
    env.ok(&["shims", "off"]);
    let zsh = env.ok(&["shell-init", "zsh"]);
    assert!(
        !zsh.contains("claude()") && zsh.contains("codex()"),
        "{zsh}"
    );
    assert!(!zsh.contains("shims"), "{zsh}");
    env.ok(&["shims", "on"]);
    assert!(!env.ok(&["shell-init", "powershell"]).contains("shims"));

    // Sourced after an rc file that put its own folders in front.
    let mut sh = env.program("bash");
    sh.arg("-c")
        .arg(format!("PATH=\"/opt/x:$PATH\"\n{bash}\necho \"$PATH\"\n"));
    let out = sh.env("PATH", "/usr/bin:/bin").output().unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout).trim(),
        format!("{}:/opt/x:/usr/bin:/bin", env.shims().display())
    );
}

#[test]
fn doctor_says_when_herdr_is_here_and_the_shims_are_not() {
    let env = Env::new();
    let gap = "agents that herdr starts by a bare name skip cswap";
    // No herdr: nothing to say.
    assert!(!env.ok(&["doctor"]).contains(gap));

    fs::create_dir_all(env.home().join(".config/herdr-app")).unwrap();
    let out = env.ok(&["doctor"]);
    assert!(out.contains(gap), "{out}");
    assert!(out.contains("cswap shims on"), "{out}");

    // On, but this environment still finds the real agent first.
    env.ok(&["shims", "on", "claude"]);
    let out = env.ok(&["doctor"]);
    assert!(!out.contains(gap), "{out}");
    assert!(
        out.contains("comes before cswap's shim on PATH here"),
        "{out}"
    );

    // On, and first on PATH.
    let out = env
        .cmd(&["doctor"])
        .env("PATH", env.shim_path())
        .output()
        .unwrap();
    let out = text(&out.stdout);
    assert!(
        out.contains("a bare claude reaches cswap from this environment"),
        "{out}"
    );

    // The link was removed by hand.
    fs::remove_file(env.shims().join("claude")).unwrap();
    let out = env.ok(&["doctor"]);
    assert!(out.contains("the shim for claude is missing"), "{out}");
}

/// herdr keeps the `PATH` it started with, so what it starts for a bare
/// `claude` is asked of herdr itself, not read from this environment.
#[test]
fn health_asks_herdr_which_program_it_would_start() {
    let env = Env::new();
    // A stand-in for `hc exec`: it runs the program with herdr's own PATH.
    let hc = env.home().join("bin/hc");
    fs::write(
        &hc,
        "#!/bin/sh\n[ \"$1\" = exec ] || exit 1\nshift 3\nPATH=$(cat \"$HOME/hc-path\") exec \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&hc, fs::Permissions::from_mode(0o755)).unwrap();
    let old = format!("{}:/usr/bin:/bin", env.home().join("bin").display());
    fs::write(env.home().join("hc-path"), &old).unwrap();
    env.ok(&["shims", "on", "claude"]);

    // This environment has the shim first, and herdr still has the old PATH.
    let run = |args: &[&str]| {
        let out = env.cmd(args).env("PATH", env.shim_path()).output().unwrap();
        (out.status.success(), text(&out.stdout))
    };
    let (_, out) = run(&["doctor"]);
    assert!(
        out.contains("a bare claude reaches cswap from this environment"),
        "{out}"
    );
    assert!(out.contains("herdr still starts ~/bin/claude"), "{out}");
    assert!(out.contains("hc server restart --fresh"), "{out}");
    let (ok, out) = run(&["shims"]);
    assert!(
        !ok && out.contains("herdr still starts ~/bin/claude"),
        "{out}"
    );

    // herdr restarted with the new PATH.
    fs::write(env.home().join("hc-path"), env.shim_path()).unwrap();
    let (_, out) = run(&["doctor"]);
    assert!(
        out.contains("herdr starts a bare claude through cswap"),
        "{out}"
    );
    let (ok, out) = run(&["shims"]);
    assert!(
        ok && out.contains("herdr starts a bare claude through cswap"),
        "{out}"
    );
}

#[test]
fn uninstall_undoes_what_cswap_wrote_outside_its_folder() {
    let env = Env::new();
    env.add_claude("work", "work@example.com");
    let bashrc = env.home().join(".bashrc");
    let mine = "alias ll='ls -l'\n";
    fs::write(
        &bashrc,
        format!(
            "{mine}\n# cswap: run the agents through cswap. Choose which in `cswap`, Settings > Shell.\n\
             command -v cswap >/dev/null 2>&1 && eval \"$(cswap shell-init bash)\"\nexport A=1\n"
        ),
    )
    .unwrap();
    env.ok(&["shims", "on"]);
    assert!(env.home().join(".profile").exists());

    // It asks first, and with nobody to ask it does nothing.
    assert!(env.fails(&["uninstall"]).contains("-y"));
    assert!(env.shims().exists());

    let out = env.ok(&["uninstall", "-y"]);
    assert!(
        out.contains("~/.bashrc") && out.contains("~/.profile"),
        "{out}"
    );
    assert!(out.contains("rm -rf ~/.cswap"), "{out}");
    assert!(!env.shims().exists());
    assert!(!env.home().join(".profile").exists());
    assert_eq!(
        fs::read_to_string(&bashrc).unwrap(),
        format!("{mine}export A=1\n")
    );
    // The accounts are the user's to delete.
    assert!(env.account_dir("claude-1").is_dir());
    assert_eq!(env.list().len(), 1);
    // A second run finds nothing left to undo.
    let out = env.ok(&["uninstall", "-y"]);
    assert!(!out.contains("~/.bashrc"), "{out}");
}
