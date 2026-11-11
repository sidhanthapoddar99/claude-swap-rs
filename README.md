# cswap

One place for the accounts of your coding agents: **Claude Code**, **Codex**, **Grok Build** and **Antigravity** (`agy`).

- Keep several logins per agent and start any of them with one flag: `claude --cswap=work`.
- See every account's limits, when each week resets, and which resets you still hold.
- Get told which account to use next, so a week that is about to reset is not wasted.
- Settings, history, skills and plugins are shared by every account of an agent. Only the login differs.

`cswap` is a single Rust binary for Linux, macOS and Windows. Starting an agent through it adds a few milliseconds and no network call. The agent then runs as itself: cswap replaces its own process with the agent's, so signals, exit codes and the terminal are the agent's own.

```
  ◆ cswap  v0.7.0                                            updated 3m ago  ·  next in 10m     Wed 11 Nov  09:09

╭───────────────────────╮
│ ▌ Overview          1 │  Overview   what to use next, and everything at a glance
│   Sync              2 │ ╭ Accounts ──────────╮╭ Running now ───────╮╭ Next reset ────────╮╭ Attention ────────╮
│   Usage             3 │ │ 6 signed in        ││ nothing            ││ in 8h 59m          ││ 2 to look at      │
│   Accounts          4 │ │ 7 in all · 4 agents││ no agent is running││ work · 62% left    ││ codex-1 is not s… │
│   Resets         1  5 │ ╰────────────────────╯╰────────────────────╯╰────────────────────╯╰───────────────────╯
│   Analytics         6 │ ╭ Use next ───────────────────────────────────────────────────────────────────────────╮
│   Health         2  7 │ │  ● Claude Code  ★ work     62% of the week left, resets in 8h 59m claude --cswap=work │
│   Settings          8 │ │  ● Codex        ★ native   78% of the week left, resets in 4d 23h               codex │
│                       │ │  ● Grok Build   ★ native   53% of the month left, resets in 16d 15h              grok │
│                       │ ╰─────────────────────────────────────────────────────────────────────────────────────╯
│                       │ ╭ Accounts ───────────────────────────────────────────────────────────────────────────╮
│                       │ │    ACCOUNT            PLAN       THIS WEEK                     RESETS IN STATE        │
│                       │ │ ▌● native claude ★    Max 5x     ━━━━━━━━━━━━━━━━━━━╋━━━  91%  1d 5h     ● ready      │
│  AGENTS               │ │  ● work claude        Max 20x    ━━━━━━━━━━━━━━━━━━━━━╋━  38%  8h 59m    ● ready      │
│  ● Claude Code     3  │ │  ● side claude        Pro        ━━━━━━━━━━━━━╋━━━━━━━━━ 100%  2d 21h    ● limit hit  │
│  ● Codex           2  │ │  ● native codex ★     Plus       ━━━━━╋━━━━━━━━━━━━━━━━━  22%  4d 23h    ● ready      │
│  ● Grok Build      1  │ │  ● native grok ★      SuperGrok  ━━━━━━━━━━╋━━━━━━━━━━━━  47%  16d 15h   ● ready      │
╰───────────────────────╯ ╰─────────────────────────────────────────────────────────────────────────────────────╯
   ⏎  actions   o  open   d  default   a  add account   r  refresh                             ? keys   esc  quit
```

## Install

**Linux and macOS**

```bash
curl -fsSL https://raw.githubusercontent.com/sidhanthapoddar99/claude-swap-rs/main/install.sh | bash
```

**Windows** (PowerShell)

```powershell
irm https://raw.githubusercontent.com/sidhanthapoddar99/claude-swap-rs/main/install.ps1 | iex
```

Both download the latest release, check its SHA-256, install the binary, and ask before they add the shell integration. `install.sh` takes `--yes`, `--no-modify-rc` and `--dir <path>`. `install.ps1`, saved to a file, takes `-Profile`, `-NoProfile`, `-NoModifyPath` and `-Dir <path>`.

**From source**

```bash
cargo install --git https://github.com/sidhanthapoddar99/claude-swap-rs
```

**Update**

```bash
cswap update          # install the latest release
cswap update --check  # only look
```

cswap looks for a newer release at most once every 24 hours, and only tells you. It never looks when it starts an agent. Turn the check off in Settings, or set `CSWAP_NO_UPDATE_CHECK=1`.

### Shell integration

With it, the plain commands `claude`, `codex`, `agy` and `grok` go through cswap. Without it, type `cswap claude`, `cswap codex` and so on.

```bash
eval "$(cswap shell-init zsh)"        # ~/.zshrc; use bash in ~/.bashrc
cswap shell-init fish | source        # ~/.config/fish/config.fish
```

```powershell
Invoke-Expression (& cswap shell-init powershell | Out-String)   # $PROFILE
```

`command claude` always skips cswap.

## Five-minute tour

```bash
cswap                     # open the dashboard
cswap add claude work     # add a second Claude account named "work", then sign in
claude --cswap=work       # start Claude Code on it
claude                    # start it on the default account, as before
cswap default claude work # make "work" the default
cswap suggest             # which account to use next, and why
```

Nothing changes until you add an account. Your existing login in `~/.claude`, `~/.codex`, `~/.gemini` or `~/.grok` shows up as account `0`, called `native`. cswap reads it and never writes to it.

## How accounts work

Each agent keeps all its state in one directory, and each has an official way to point it somewhere else:

| Agent | Command | Its own dir | Moved with |
| --- | --- | --- | --- |
| Claude Code | `claude` | `~/.claude` | `CLAUDE_CONFIG_DIR` |
| Codex | `codex` | `~/.codex` | `CODEX_HOME` |
| Grok Build | `grok` | `~/.grok` | `GROK_HOME` |
| Antigravity | `agy` | `~/.gemini` | `--gemini_dir=` (experimental) |

A cswap account is a directory of its own that mirrors the agent's dir with links. Only the login and a few per-machine files are real:

```
~/.cswap/
  settings.json                     cswap's own settings
  harness/claude/settings.json      per agent: default account, wrapping, sync
  accounts/
    claude-1/
      settings.json                 alias, identity, plan, last refresh and its data
      config_dir/                   what the agent is pointed at
        .credentials.json           real: this account's login
        .claude.json                real: who it is
        settings.json  -> ~/.claude/settings.json     \
        CLAUDE.md      -> ~/.claude/CLAUDE.md          |  links, made by sync
        projects/      -> ~/.claude/projects           |
        skills/        -> ~/.claude/skills            /
    codex-1/ …
```

What follows from this:

- **Install once, use everywhere.** A plugin, skill or MCP server set up under one account is there under every account of that agent.
- **History is shared.** Hit a limit, switch account, resume the same conversation.
- **New files are picked up by themselves.** Sync looks at the agent's dir before every launch, so a file a future version invents is linked without a cswap release.
- **The agent's own login is left alone.** cswap never writes into the agent's own dir. Your editor extension and anything else that reads it keep working.

### Sync

Sync runs before every launch and from the dashboard's Sync page. It links whatever is new, drops links whose target is gone, and reports **shadowed** entries: a real file in an account where a link should be, which means the agent wrote something there that is not shared. `cswap sync --repair` moves such files to `~/.cswap/parked/` and puts the link back. Nothing is deleted.

Each agent has a short built-in list of names that stay private (logins, identity, locks, caches). Add your own per agent in Settings.

## Starting agents

```bash
cswap claude [args…]            # the default account
cswap claude --cswap=work       # by alias
cswap claude --cswap=2          # by number
cswap claude --cswap=native     # the agent's own login
cswap claude --cswap=auto       # the best account right now
cswap claude --cswap=work,side  # the first of these that is free
cswap claude --cswap-select     # pick from a list
cswap start                     # pick the agent, then the account
cswap start --default           # pick the agent, use its default account
```

Everything after the agent's name goes to the agent untouched, `--help` and `-h` included. The only arguments cswap takes out are `--cswap=…` and `--cswap-select`.

Which account a launch uses, first match wins:

1. `--cswap=<account>` on the command line
2. `CSWAP_CLAUDE`, `CSWAP_CODEX`, `CSWAP_AGY` or `CSWAP_GROK` in the environment (`export CSWAP_CLAUDE=work` pins one terminal)
3. the agent's default: `native`, `auto` or an account, set with `cswap default` or in Settings

An account is named by its alias (`work`), its key (`claude-2`), its number within the agent (`2`), its email, or `native`.

`cswap ps` lists the agents that are running and which account each is on.

## The dashboard

Run `cswap` with no arguments. Keys: `1`–`8` or the arrow keys to move, `Enter` to act, `?` for every key. `Esc` steps back: out of a dialog, out of the page, then out of cswap. The mouse works too.

| Page | What it shows |
| --- | --- |
| **Overview** | What to use next for each agent, and every account on one screen. |
| **Sync** | Per account: what is linked, what is private, what is shadowed, and when it was last synced. |
| **Usage** | One card per account: the weekly limit with a pace marker, the day and time it resets, extra usage, spare resets. |
| **Accounts** | Add, sign in, alias, set default, remove. Plan, renewal date, how long the login is valid, where it lives. |
| **Resets** | Four parts: everything coming up in one list by time, the spare resets you hold and when they expire, when each weekly limit resets, and when each plan renews. |
| **Analytics** | Burn rate per account over the last week and where it lands at the reset. More is planned. |
| **Health** | Anything that needs attention: signed-out accounts, shadowed files, a missing shell integration. |
| **Settings** | General settings, one tab per agent, and the shell line to copy. |

Usage is fetched when the dashboard opens and then every 10 minutes while it stays open (change it in Settings). Nothing is fetched in the background when the dashboard is closed, and starting an agent never fetches.

**Adding an account** from the Accounts page: press `a`, pick the agent, type an alias or leave it empty, press `Enter`. cswap steps aside and the agent's own sign-in runs in your terminal. When it finishes you are back in the dashboard with the new account. If the sign-in is abandoned, nothing is added.

### Resets are view only

cswap shows the resets an account holds and when they expire. It cannot spend one, and it never calls a vendor route that does. Spend a reset in the agent or on the vendor's site.

### Which account to use next

The suggestion follows one idea: use what you would otherwise lose. An account whose week resets soon with room left goes first, and so does a cancelled plan with days left. An account that is at its limit, signed out, or whose plan has ended goes last. `cswap suggest` prints the ranking with the reason for each line, and `--cswap=auto` uses the top one.

## Commands

```
cswap                      open the dashboard
cswap <agent> [args…]      run an agent on its default account
cswap start [--default]    pick an agent and an account, then run it

ps                         which agents are running, on which account
list                       every account and its state        (--json)
usage [account]            limits, pace and reset times       (--refresh)
resets                     spare resets and what resets when
suggest [agent]            which account to use next, and why

add <agent> [alias]        add an account and sign in
login <account>            sign in again
alias <account> [name]     name an account, or clear its name
default <agent> [account]  show or set what a bare launch uses
remove <account>           delete an account's login          (--yes)

refresh [account]          fetch usage now
sync [account]             bring links up to date             (--check, --repair)
doctor                     check for problems
migrate                    move cswap 0.6 profiles into ~/.cswap
shell-init <shell>         print shell integration
update                     install the latest release         (--check)
```

`cswap -h` prints this with examples. `cswap <command> -h` explains one command.

## What each agent reports

| | Limits | Spare resets | Plan | Renewal date |
| --- | --- | --- | --- | --- |
| Claude Code | session, week, per-model week, extra usage | count and expiry | yes | set by hand |
| Codex | session, week | count and expiry | yes | yes |
| Grok Build | monthly credits | not reported | yes | not reported |
| Antigravity | week and 5-hour of the Gemini models, through `agy /usage` | not reported | not reported | not reported |

Where the vendor does not report a renewal date or a reset, you can note it by hand on the Accounts page.

## What cswap does and does not do

- It **never renews a token**. Each agent renews its own login when it runs. An account you have not started for a while can show its last known usage until you start it or sign in again.
- It **never writes** into an agent's own config dir.
- It calls **only the endpoints the vendor's own CLI calls** for usage: `api.anthropic.com`, `chatgpt.com` and `cli-chat-proxy.grok.com`, each with that account's own login. For Antigravity it runs `agy` itself. Claude lists spare resets only to its CLI, so cswap identifies as the CLI for that one read. Turn that off per agent in Settings (`cli_identity`) and the reset details are skipped.
- It contacts `api.github.com` once a day to look for a new release, unless you turn that off.
- No telemetry.

Logins are stored the way each agent stores them, in files only you can read (`0700` directories, `0600` files).

## Coming from cswap 0.6

0.6 handled Claude Code only and kept profiles in `~/.local/share/cswap`.

```bash
cswap migrate
```

Each old profile becomes a Claude account and keeps its aliases. The profile's directory is moved, and a link is left at the old path so that paths recorded by Claude still resolve.

| 0.6 | now |
| --- | --- |
| `cswap login <name>` | `cswap add claude <name>` |
| `cswap activate <name>` | `export CSWAP_CLAUDE=<name>` |
| `cswap run <name>` | `cswap claude --cswap=<name>` |
| `cswap watch` | `cswap` (the dashboard) |
| `cswap upgrade` | `cswap update` (`upgrade` still works) |

## Platforms

| | Status |
| --- | --- |
| Linux | Developed and tested here. |
| macOS | Built in CI. Claude Code keeps its login in the keychain there, which cswap reads with `/usr/bin/security`. Not yet tested on a real Mac. |
| Windows | Built in CI. The agent runs as a child process, since Windows cannot replace a process. Linking needs Developer Mode or an elevated shell. Not yet tested on a real PC. |

`cswap ps` reads `/proc` on Linux and sees every session. On macOS and Windows it lists the sessions that were started through cswap.

## Environment

| Variable | Effect |
| --- | --- |
| `CSWAP_HOME` | where cswap keeps its data (default `~/.cswap`) |
| `CSWAP_CLAUDE`, `CSWAP_CODEX`, `CSWAP_AGY`, `CSWAP_GROK` | the account a launch uses in this environment |
| `CSWAP_CLAUDE_BIN`, … | the real binary, when it is not the first on `PATH` |
| `CSWAP_NO_UPDATE_CHECK` | never look for a newer release |
| `NO_COLOR` | plain text output |
| `CSWAP_ACCOUNT` | set by cswap for the agent it starts: the account's key |

## Uninstall

Remove the shell integration line from your rc file, delete the binary, and delete `~/.cswap`. The accounts' logins live there, so sign in to anything you still need first. The agents' own directories were never changed.

## Develop

```bash
cargo test                                   # unit tests and end-to-end tests of the binary
cargo clippy --all-targets -- -D warnings
CSWAP_SHOW=1 cargo test every_page -- --nocapture   # print every dashboard page
```

Tests run in a fabricated home with shell scripts standing in for the agents. They never touch your logins or the network.

Inspired by [claude-swap](https://github.com/realiti4/claude-swap).

## License

See [LICENSE](LICENSE).
