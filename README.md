# claude-swap-rs

Fast multi-account switcher for Claude Code, in a single static Rust binary (`cswap`).

Named accounts, per-terminal activation, parallel sessions, live usage dashboard — with one shared conversation history across all accounts (isolatable per account). ~5 ms overhead per launch; `cswap` `exec`s the real `claude`, so signals, exit codes, and interactivity are native.

Linux / WSL / macOS. Inspired by [claude-swap](https://github.com/realiti4/claude-swap) (Python).

## How it works

Claude Code keeps all state in `~/.claude` (plus `~/.claude.json`), and the official `CLAUDE_CONFIG_DIR` env var redirects both. Only **two files** in that entire tree define *who you are*: `.credentials.json` and the `oauthAccount` key of `.claude.json`.

So a cswap profile is your `~/.claude` wearing a different identity card:

```
~/.local/share/cswap/profiles/work/
  .credentials.json      real file — work's tokens (0600)
  .claude.json           real file — work's identity + onboarding seed (0600)
  settings.json          -> ~/.claude/settings.json     \
  CLAUDE.md              -> ~/.claude/CLAUDE.md          |  everything else is a
  plugins/               -> ~/.claude/plugins             |  symlink, auto-discovered
  projects/              -> ~/.claude/projects            |  on every launch
  history.jsonl          -> ~/.claude/history.jsonl      /
```

Consequences:

- **Settings, plugins, MCP servers, skills, agents are installed once, visible everywhere** — a plugin installed while on `work` is instantly available on `personal` (one real directory on disk).
- **History is shared by default**: `claude -r` lists the same conversations on every account, so you can hit a rate limit, switch accounts, and resume the same conversation. Transcripts are keyed by project path, never account — this matches stock Claude Code behavior. Set `isolated = true` on an account to give it its own `projects/` + `history.jsonl` instead (e.g. an employer seat).
- **Files future Claude versions invent are picked up automatically** — the symlink sync rescans `~/.claude` on every launch instead of maintaining a hardcoded list.
- **The account logged into the live `~/.claude` runs via passthrough**: no profile, no `CLAUDE_CONFIG_DIR`, cswap never touches its tokens. One credential copy means cswap can never rotate the refresh-token family out from under the login your VS Code extension uses.

cswap **never writes into `~/.claude` or `~/.claude.json`** — it only reads them. All cswap state lives in `~/.config/cswap/` and `~/.local/share/cswap/`. Uninstalling is `rm -rf` of those two directories.

Network: exactly two first-party endpoints (Anthropic's OAuth token refresh and usage API). No telemetry of any kind.

## Install

### Installer script (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/sidhanthapoddar99/claude-swap-rs/main/install.sh | bash
```

Detects your platform, downloads the latest release, verifies the SHA-256 checksum, installs to `~/.local/bin`, and **asks** before adding the shell integration to your `~/.zshrc` / `~/.bashrc` (idempotent marker block; `--yes` to skip the prompt, `--no-modify-rc` to never touch rc files, `--dir <path>` for a custom install dir).

### Manual (GitHub Releases)

```bash
curl -fsSL https://github.com/sidhanthapoddar99/claude-swap-rs/releases/latest/download/cswap-x86_64-unknown-linux-musl.tar.gz \
  | tar xz -C ~/.local/bin
# macOS (Apple Silicon): cswap-aarch64-apple-darwin.tar.gz
# macOS (Intel):         cswap-x86_64-apple-darwin.tar.gz
# Linux ARM64:           cswap-aarch64-unknown-linux-musl.tar.gz
```

### From source

```bash
cargo install --git https://github.com/sidhanthapoddar99/claude-swap-rs
```

### Shell integration (required for `activate` and the `claude` wrapper)

The installer offers to set this up; manually, add to `~/.zshrc` or `~/.bashrc`:

```bash
eval "$(cswap shell-init zsh)"   # or: bash
```

This defines two functions: `cswap` (so `activate` can export into the current shell) and `claude` (which routes through cswap so the active/default account applies). `command claude` always bypasses everything.

### Updating

```bash
cswap upgrade
```

Downloads the latest release, verifies its checksum, and atomically replaces the binary. `cswap list` also nudges (at most once per 24h, cached, only in interactive terminals) when a newer version exists — set `CSWAP_NO_UPDATE_CHECK=1` to disable.

## Usage

```bash
# Register the account you're already logged into (keyed by its email):
cswap login                     # offers an optional alias right away

# Add MORE accounts from inside cswap (recommended):
cswap login --new --alias work  # launches claude in a clean staging profile;
                                # log in as the new account, /exit, done.
                                # Your current login is never touched.

cswap list             # Default/Active entity lines + profiles with one
                       # colored line per usage window (5h / 7d / per-model)
cswap list --quick     # skip the usage API calls

# Anywhere an account is expected: pass an alias or email — or pass nothing
# on a terminal and pick from an interactive menu.
cswap activate         # interactive picker (this shell only)
cswap run              # interactive picker, one-off run
cswap default          # interactive picker

cswap alias list
cswap alias create     # pick account, type alias (or: cswap alias create work w)
cswap alias remove     # pick from a menu       (or: cswap alias remove w)

cswap remove           # interactive picker + confirmation (--yes to skip)

cswap default work     # what bare `claude` uses everywhere
cswap activate personal  # what `claude` uses in THIS terminal only
cswap activate         # back to default

claude                 # runs as active/default account — all flags pass through
claude -r              # same shared history from any account
cswap run work -r      # one-off as a specific account, ignoring active/default

cswap watch            # live usage dashboard (refreshes every 300s)
cswap watch -i 120

cswap remove old-account   # forget it (never touches ~/.claude data)
```

### Configuration

`~/.config/cswap/config.toml` — written by `cswap login` / `cswap default`, editable by hand:

```toml
default = "you@gmail.com"   # always the email

[[account]]
email = "you@gmail.com"     # the unique identity — there is no separate "name"
aliases = ["personal", "p"] # the labels you type; all resolve everywhere
isolated = false

[[account]]
email = "you@corp.com"
aliases = ["work"]
isolated = true    # own projects/ + history.jsonl — not shared with other accounts
```

Pre-0.3.1 configs (with `name = ...`) migrate automatically on the first run: the name becomes the primary alias and store/profile files are re-keyed by email.

### Notes

- **Parallel accounts:** activate different accounts in different terminals and run them simultaneously. Separate config dirs; shared history via symlinks.
- **Trust & MCP carry-over:** a new profile seeds `mcpServers` and per-project trust (`projects` key) from your live `~/.claude.json` once at creation, so you don't re-approve every repo. After that the copies evolve independently.
- **Token refresh:** before launching a profile account, cswap refreshes its OAuth token if it expires within 5 minutes and persists the rotation. The live-`~/.claude` account is never refreshed by cswap — that's Claude's job.
- **Windows:** not supported (symlink + exec semantics differ). WSL works fully.

## Development

```bash
git clone git@github.com:sidhanthapoddar99/claude-swap-rs.git
cd claude-swap-rs
cargo test                 # unit + end-to-end (runs against a fabricated $HOME; never touches yours)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Branch flow: work on `dev`, PR into `main`. CI (fmt, clippy, tests) runs on every push and PR.

## Publishing a release

Releases are built by CI from version tags on `main`:

```bash
# 1. bump version in Cargo.toml (on dev), PR into main
# 2. tag the merge commit on main:
git checkout main && git pull
git tag v0.2.0
git push origin v0.2.0
```

The `Release` workflow builds static binaries for Linux (x86_64/aarch64 musl) and macOS (x86_64/aarch64), and publishes them to GitHub Releases with SHA-256 checksums and auto-generated notes.

## License

MIT
