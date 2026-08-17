# claude-swap-rs

Fast multi-profile switcher for Claude Code, in a single static Rust binary (`cswap`).

Named profiles, per-terminal activation, parallel sessions, live usage dashboard — with one shared conversation history across all of them. ~5 ms overhead per launch; `cswap` `exec`s the real `claude`, so signals, exit codes, and interactivity are native.

Linux / WSL / macOS. Inspired by [claude-swap](https://github.com/realiti4/claude-swap) (Python).

## The model

There are two kinds of thing, and nothing links them.

**`default`** is your live `~/.claude`. It does not know cswap exists. cswap **only reads it** — to report who is logged in, to seed a new profile's theme and trust, and to mirror it as symlinks. It has no profile directory, no aliases, and no entry in any config file. To change who it is, log in with claude itself.

**A profile** is a whole entity: its own directory, its own login, its own OAuth token family. A profile **is** an account — one profile per account, keyed by email, with aliases as the labels you type.

`default` is the one exception to that rule. It may hold the same account as a profile, because it is not a profile. Everything else is unique.

That independence only holds because no code path copies credentials between entities. One account with two live copies of its refresh-token family means whichever side ran last leaves the other holding a dead ancestor.

## How it works

Claude Code keeps all state in `~/.claude` (plus `~/.claude.json`), and the official `CLAUDE_CONFIG_DIR` env var redirects both. Only **two files** in that entire tree define *who you are*: `.credentials.json` and the `oauthAccount` key of `.claude.json`.

So a profile is your `~/.claude` wearing a different identity card:

```
~/.local/share/cswap/profiles/you@corp.com/
  .credentials.json      real file — this account's tokens, the only copy (0600)
  .claude.json           real file — its identity + onboarding seed (0600)
  settings.json          -> ~/.claude/settings.json     \
  CLAUDE.md              -> ~/.claude/CLAUDE.md          |  everything else is a
  plugins/               -> ~/.claude/plugins             |  symlink, auto-discovered
  projects/              -> ~/.claude/projects            |  on every launch
  history.jsonl          -> ~/.claude/history.jsonl      /
```

Consequences:

- **Settings, plugins, MCP servers, skills, agents are installed once, visible everywhere** — a plugin installed under one profile is instantly available under every other (one real directory on disk).
- **A profile directory never moves.** It is named for the account it holds. That matters because claude records absolute paths into `~/.claude` — plugin install locations, session aliases — which would dangle if profiles were renamed.
- **History is shared**: `claude -r` lists the same conversations from every profile, so you can hit a rate limit, switch, and resume the same conversation. Transcripts are keyed by project path, never by account — this matches stock Claude Code behavior.
- **Files future Claude versions invent are picked up automatically** — the symlink sync rescans `~/.claude` on every launch instead of maintaining a hardcoded list.
- **`default` runs as passthrough**: no profile, no `CLAUDE_CONFIG_DIR`, so cswap never touches its tokens and your VS Code extension keeps working. It is reached by *selecting* it, never by matching emails.

Three names are never linked into a profile:

| never linked | why |
| --- | --- |
| `.credentials.json` | identity — the profile's own tokens |
| `backups/` | holds `.claude.json.backup.<ms>`. `.claude.json` is per-profile, so its backups must be too, or a profile can restore the live account's identity file |
| `.git/` | a version-controlled `~/.claude` would treat the profile as its working tree — one `git add -A` inside a profile rewrites your tracked `~/.claude` |

cswap **never** writes into `~/.claude` or `~/.claude.json`. All cswap state lives in `~/.config/cswap/` and `~/.local/share/cswap/`; uninstalling is `rm -rf` of those two directories.

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

This defines two functions: `cswap` (so `activate` can export into the current shell) and `claude` (which routes through cswap so the active profile applies). `command claude` always bypasses everything.

### Updating

```bash
cswap upgrade
```

Downloads the latest release, verifies its checksum, and atomically replaces the binary. `cswap list` also nudges (at most once per 24h, cached, only in interactive terminals) when a newer version exists — set `CSWAP_NO_UPDATE_CHECK=1` to disable.

## Usage

```bash
# Create a profile by logging into it. cswap builds the directory, links it to
# ~/.claude, then hands it to claude with no credentials so claude walks its own
# login. /exit when done. Your ~/.claude login is never touched.
cswap login you@corp.com              # new profile for that account
cswap login you@corp.com --alias work # ...and label it immediately
cswap login work                      # re-login an existing profile

cswap list             # the default line, then one row per profile:
                       # status, name, account, aliases, 5h/7d gates
cswap list --quick     # skip the usage API calls

# Anywhere a profile is expected: pass its account, an alias, or `default` —
# or pass nothing on a terminal and pick from an interactive menu.
cswap activate         # interactive picker (this shell only)
cswap run              # interactive picker, one-off run

cswap activate work    # what `claude` uses in THIS terminal only
cswap activate default # back to the live ~/.claude
cswap default          # who is the live ~/.claude? (read-only)

cswap alias list
cswap alias create     # pick profile, type alias (or: cswap alias create work w)
cswap alias remove     # pick from a menu        (or: cswap alias remove w)

claude                 # runs as the active profile — all flags pass through
claude -r              # the same shared history from any profile
cswap run work -r      # one-off as a specific profile, ignoring what's active
cswap run default      # one-off against the live ~/.claude

cswap usage            # bars, per-model windows and reset times
cswap usage work       # just one profile
cswap usage default    # just the live ~/.claude

cswap watch            # the same view, redrawn every 300s ([r] refresh, [q] quit)
cswap watch -i 120

cswap remove old       # forget a profile (never touches ~/.claude data)
```

You name the account up front because it keys the directory, and that directory has to exist and be linked *before* claude runs. cswap checks that the account which arrives is the one you asked for, and refuses to file one account's tokens under another's name.

Re-running `cswap login` on an existing profile (by email or alias) logs it in again. Its current tokens are discarded first, or claude would see a live session and never offer the login — so it confirms (`--yes` to skip).

### Configuration

`~/.config/cswap/config.toml` — written by `cswap login`, editable by hand:

```toml
[[profile]]
email = "you@gmail.com"     # the key: identifies the profile and names its directory
aliases = ["personal", "p"] # the labels you type; all resolve everywhere

[[profile]]
email = "you@corp.com"
aliases = ["work"]
```

One profile per account. `default` appears nowhere in this file and never can — it is the live `~/.claude`, and it is the only thing permitted to hold an account a profile already holds.

### Migrating

The first 0.6.1 run rewrites the config in place and says what it did:

- `[[account]]` becomes `[[profile]]`. Both were already keyed by email, so **no directory moves** and `CSWAP_ACTIVE` in open terminals keeps working.
- The `accounts/` credential store moves to `accounts.pre-0.6.bak`. **Nothing is copied out of it into a profile** — that would give one account two token families. A profile that only ever lived in the store (typically the one that was your live login) comes out with no credentials, and cswap tells you to run `cswap login <email>`.
- If you passed through 0.6.0, which briefly keyed profiles by a separate `name`: the name folds back into the alias list and `profiles/<name>` moves back under `profiles/<email>`.

`cswap default <account>` is gone. The default is not a pointer at a profile, so there is nothing to swap: use `cswap activate <name>` for a terminal, `cswap run <name>` for one command, or claude's own `/login` to change `~/.claude` itself.

### Notes

- **Parallel profiles:** activate different profiles in different terminals and run them simultaneously. Separate config dirs; shared history via symlinks.
- **Trust & MCP carry-over:** a new profile seeds `mcpServers` and per-project trust (`projects` key) from your live `~/.claude.json` once at creation, so you don't re-approve every repo. After that the copies evolve independently. No identity is copied — claude records that itself when the profile's own login completes.
- **`cswap activate` is per shell, but an IDE may clone it.** VS Code applies a stored environment to every terminal it opens, so an export can outlive the shell you made it in. `cswap activate default` clears one terminal; reloading the window clears the rest.
- **Token refresh:** before launching a profile, cswap refreshes its OAuth token if it expires within 5 minutes and persists the rotation. `default` is never refreshed by cswap — that's Claude's job.
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
git tag v0.6.1
git push origin v0.6.1
```

The `Release` workflow builds static binaries for Linux (x86_64/aarch64 musl) and macOS (x86_64/aarch64), and publishes them to GitHub Releases with SHA-256 checksums and auto-generated notes.

## License

MIT
