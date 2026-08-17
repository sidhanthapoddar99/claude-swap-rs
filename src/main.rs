mod cmds;
mod config;
mod interactive;
mod oauth;
mod paths;
mod profile;
mod ui;
mod update_check;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cswap",
    version,
    about = "Fast multi-profile switcher for Claude Code",
    long_about = "Fast multi-profile switcher for Claude Code.\n\n\
A PROFILE is a whole entity: its own directory, its own login, its own\n\
tokens. It IS an account — one profile per account, keyed by email, with\n\
aliases as the labels you type.\n\n\
`default` is the live ~/.claude — a separate entity that does not know cswap\n\
exists. cswap only READS it, never writes it, and it is the ONE thing that\n\
may hold the same account as a profile. All cswap state lives in\n\
~/.config/cswap and ~/.local/share/cswap.",
    arg_required_else_help = true,
    // clap 4 can't group subcommands into sections, so the command list is
    // hand-written here instead of via {subcommands}. Keep these one-liners
    // in sync with each variant's doc comment (what `cswap help <cmd>` shows).
    help_template = "\
{about-with-newline}
{usage-heading} {usage}

Setup:
  shell-init  Print shell integration (bash|zsh) — eval it from your rc file
  login       Create a profile by logging into it: cswap login <email>
  upgrade     Self-update from GitHub Releases

Profiles:
  list        Table of the default and every profile: status, account, usage
  alias       Manage aliases: list, create, remove
  default     Show who the live ~/.claude is (read-only)
  remove      Forget a profile (menu when no argument; always confirms)

Session:
  activate    Set the profile for THIS terminal (menu when no argument)
  run         Run claude as a profile: cswap run [PROFILE] [CLAUDE_ARGS]...

Limits:
  usage       Detailed per-profile usage with bars and reset times
  watch       Live usage dashboard (redraws every INTERVAL seconds)

Options:
{options}
{after-help}",
    after_help = "\
QUICK START:
  1. eval \"$(cswap shell-init zsh)\"   # add to ~/.zshrc (or bash), once
  2. cswap login you@corp.com --alias work   # log into a profile
  3. cswap login you@gmail.com --alias home  # ...and another

  cswap activate work       use that profile in THIS terminal
  cswap activate default    back to the live ~/.claude
  cswap list                see everything at a glance
  cswap usage               see the full picture

`default` is your existing ~/.claude, untouched. cswap never writes there, so
changing it means logging in with claude itself.

Run `cswap help <command>` for details and examples of each command."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    // ---------------------------------------------------------------- Setup
    /// Print shell integration (bash|zsh) — eval it from your rc file
    #[command(
        name = "shell-init",
        long_about = "Print the shell integration snippet.\n\n\
Defines two functions:\n  \
cswap()   intercepts `cswap activate` so the export lands in this shell\n  \
claude()  routes bare `claude` through cswap (active/default account)\n\n\
`command claude` always bypasses both.\n\n\
SETUP (once, in ~/.zshrc or ~/.bashrc):\n  eval \"$(cswap shell-init zsh)\""
    )]
    ShellInit { shell: String },

    /// Create a profile by logging into it: cswap login <email>
    #[command(
        long_about = "Build the profile directory, then launch claude inside it with\n\
no credentials so claude walks its own login. Exit claude (/exit) and cswap\n\
checks that the account which arrived is the one you asked for.\n\n\
You name the account up front because it keys the directory, and that\n\
directory has to exist and be linked BEFORE claude runs. One profile per\n\
account: `default` may hold the same account, nothing else may.\n\n\
Every profile logs in for itself. cswap never copies credentials out of\n\
~/.claude or between profiles — one account with two copies of its\n\
refresh-token family means whichever side ran last leaves the other holding\n\
a dead ancestor.\n\n\
Naming an existing profile (by email or alias) logs it in again, after a\n\
confirmation: its current tokens are discarded first, or claude would see a\n\
live session and never offer the login.\n\n\
EXAMPLES:\n  cswap login you@corp.com                # new profile\n  \
cswap login you@corp.com --alias work   # ...and label it immediately\n  \
cswap login work                        # re-login an existing profile"
    )]
    Login {
        /// Account email, or an existing profile (asked for when omitted)
        key: Option<String>,
        /// Alias to attach to the profile
        #[arg(long)]
        alias: Option<String>,
        /// Skip the confirmation when re-logging in an existing profile
        #[arg(long)]
        yes: bool,
    },

    /// Self-update from GitHub Releases
    #[command(
        long_about = "Download the latest release for this platform, verify its\n\
SHA-256 checksum, and atomically replace this binary. `cswap list` also\n\
nudges (at most once per 24h) when a newer version exists; set\n\
CSWAP_NO_UPDATE_CHECK=1 to disable that check."
    )]
    Upgrade,

    // ------------------------------------------------------------- Accounts
    /// Table of the default and every profile: status, account, usage
    #[command(
        long_about = "The default on its own line, then one borderless row per\n\
profile — which is active, the account behind it, its aliases, and the 5h/7d\n\
gates. Percentages are colored <70 green, <90 yellow, else red.\n\n\
For bars, per-model windows and reset times, use `cswap usage`.\n\n\
EXAMPLES:\n  cswap list             # with usage (one API call per profile)\n  \
cswap list --quick     # instant, no network"
    )]
    List {
        /// Skip the usage API calls
        #[arg(short, long)]
        quick: bool,
    },

    /// Manage aliases: list, create, remove (interactive when args omitted)
    #[command(
        long_about = "A profile IS its account, keyed by email; an alias is a shorter\n\
Aliases resolve everywhere a profile is referenced. `default` is a reserved\n\
word, not a profile, so it has no aliases.\n\n\
EXAMPLES:\n  cswap alias list\n  cswap alias create            # pick profile, type alias\n  \
cswap alias create work w     # scripted\n  cswap alias remove            # pick from a menu\n  \
cswap alias remove w"
    )]
    Alias {
        #[command(subcommand)]
        action: AliasCmd,
    },

    /// Show who the live ~/.claude is (read-only)
    #[command(
        long_about = "Report which account is logged into the live ~/.claude, and\n\
which profiles happen to hold that same account independently.\n\n\
`default` is its own entity. It is not a pointer at a profile, no profile is\n\
a pointer at it, and cswap only ever READS it. There is nothing to swap here:\n\
to change who ~/.claude is, log in with claude itself.\n\n\
EXAMPLES:\n  cswap default             # who is the live ~/.claude?\n  \
cswap activate work       # use a profile in this terminal instead"
    )]
    Default {
        /// Accepted only to explain that swapping is gone
        #[arg(hide = true)]
        key: Option<String>,
    },

    /// Forget a profile (menu when no argument; always confirms)
    #[command(
        long_about = "Remove a profile: its config entry and its directory — after a\n\
confirmation (skip with --yes). That directory holds the profile's only\n\
credentials, so you would have to log in again.\n\n\
The directory is mostly symlinks into ~/.claude and they are removed without\n\
being followed, so your real Claude data (history, settings, plugins) is\n\
never touched. `default` cannot be removed: cswap owns no state for it."
    )]
    Remove {
        /// Account or alias (interactive menu when omitted)
        key: Option<String>,
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },

    // -------------------------------------------------------------- Session
    /// Set the profile for THIS terminal (menu when no argument)
    #[command(
        long_about = "Set the active profile for THIS terminal only. With no\n\
argument, an interactive menu lists `default` and every profile. Requires the\n\
shell integration (eval \"$(cswap shell-init zsh)\"). Other terminals are\n\
unaffected; new terminals start on the default.\n\n\
EXAMPLES:\n  cswap activate            # interactive picker\n  \
cswap activate you@corp.com  # by account\n  cswap activate w          # by alias\n  \
cswap activate default    # back to the live ~/.claude"
    )]
    Activate {
        /// Account, alias, or `default` (menu when omitted)
        key: Option<String>,
        /// Emit the export line for the shell wrapper to eval
        #[arg(long, hide = true)]
        print: bool,
    },

    /// Run claude as a profile: cswap run [NAME] [CLAUDE_ARGS]...
    #[command(
        long_about = "Run claude once as a specific profile, ignoring what this\n\
terminal activated. The first argument is treated as a target only when it\n\
matches an account, an alias, or `default`; everything else passes to\n\
claude verbatim. With no arguments on a terminal, a picker asks.\n\
cswap exec()s the real claude binary — signals and exit codes are native.\n\n\
`default` runs against ~/.claude itself, with no CLAUDE_CONFIG_DIR, so cswap\n\
never touches its tokens. A profile always runs from its own directory — even\n\
when it holds the same account as the default.\n\n\
EXAMPLES:\n  cswap run                      # interactive picker\n  \
cswap run work -r              # resume picker, work pays\n  \
cswap run default              # explicitly the live ~/.claude\n  \
cswap run -- --model opus      # active profile, flags pass through"
    )]
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    // --------------------------------------------------------------- Limits
    /// Detailed per-profile usage with bars and reset times
    #[command(
        long_about = "A card for the default and one per profile: every window (5h,\n\
7d, per-model weekly) with a bar, percentage and reset countdown. Same source\n\
as `cswap list`, which shows only the 5h/7d numbers on one line.\n\n\
EXAMPLES:\n  cswap usage          # everything\n  \
cswap usage work     # just one profile\n  \
cswap usage default  # just the live ~/.claude"
    )]
    Usage {
        /// Account, alias, or `default` (everything when omitted)
        key: Option<String>,
    },

    /// Live usage dashboard (redraws every INTERVAL seconds)
    #[command(
        long_about = "`cswap usage`, redrawn on an interval (default 300s, minimum\n\
60 — the usage API budgets ~20-30 requests/hour per account token, so don't\n\
go much lower).\n\n\
KEYS:\n  r   refresh now\n  q / Esc / Ctrl-C   quit\n\n\
EXAMPLES:\n  cswap watch\n  cswap watch -i 120"
    )]
    Watch {
        #[arg(short, long, default_value_t = 300)]
        interval: u64,
    },

    /// Internal: what the claude() shell wrapper calls
    #[command(name = "_claude", hide = true)]
    ClaudeShim {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum AliasCmd {
    /// List every profile and its aliases
    List,
    /// Add an alias: cswap alias create [PROFILE] [ALIAS]
    Create {
        profile: Option<String>,
        alias: Option<String>,
    },
    /// Remove an alias: cswap alias remove [ALIAS]
    Remove { alias: Option<String> },
}

fn main() {
    if let Err(e) = config::migrate_on_disk() {
        eprintln!("cswap: config migration failed: {e:#}");
    }
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Login { key, alias, yes } => cmds::login::run(key, alias, yes),
        Cmd::Activate { key, print } => cmds::activate::run(key, print),
        Cmd::List { quick } => cmds::list::run(quick),
        Cmd::Default { key } => cmds::default_cmd::run(key),
        Cmd::Run { args } => cmds::run::run(args),
        Cmd::Usage { key } => cmds::usage::run(key),
        Cmd::Watch { interval } => cmds::watch::run(interval),
        Cmd::ShellInit { shell } => cmds::shell_init::run(&shell),
        Cmd::Upgrade => cmds::upgrade::run(),
        Cmd::Alias { action } => match action {
            AliasCmd::List => cmds::alias::list(),
            AliasCmd::Create { profile, alias } => cmds::alias::create(profile, alias),
            AliasCmd::Remove { alias } => cmds::alias::remove(alias),
        },
        Cmd::Remove { key, yes } => cmds::remove::run(key, yes),
        Cmd::ClaudeShim { args } => cmds::run::shim(args),
    };
    if let Err(e) = result {
        eprintln!("cswap: {e:#}");
        std::process::exit(1);
    }
}
