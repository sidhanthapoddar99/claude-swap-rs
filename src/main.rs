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
    about = "Fast multi-account switcher for Claude Code",
    long_about = "Fast multi-account switcher for Claude Code.\n\n\
Accounts are keyed by email; aliases are the labels you type. Anywhere an\n\
account is expected you can pass an alias or the email — or pass nothing on\n\
a terminal and pick from an interactive menu.\n\n\
The default is not stored, it's derived: whoever is logged into the live\n\
~/.claude IS the default. `cswap default <account>` swaps that login — the\n\
one command that writes into ~/.claude; everything else only reads it. All\n\
cswap state lives in ~/.config/cswap and ~/.local/share/cswap.",
    arg_required_else_help = true,
    // clap 4 can't group subcommands into sections, so the command list is
    // hand-written here instead of via {subcommands}. Keep these one-liners
    // in sync with each variant's doc comment (what `cswap help <cmd>` shows).
    help_template = "\
{about-with-newline}
{usage-heading} {usage}

Setup:
  shell-init  Print shell integration (bash|zsh) — eval it from your rc file
  login       Register the current claude login; --new logs into a fresh account
  upgrade     Self-update from GitHub Releases

Accounts:
  list        Table of all accounts: status, email, aliases, usage
  alias       Manage aliases: list, create, remove
  default     Show or swap the default account (the live ~/.claude login)
  remove      Forget an account (menu when no argument; always confirms)

Session:
  activate    Set the account for THIS terminal (menu when no argument)
  run         Run claude as an account: cswap run [ALIAS|EMAIL] [CLAUDE_ARGS]...

Limits:
  usage       Detailed per-account usage with bars and reset times
  watch       Live usage dashboard (redraws every INTERVAL seconds)

Options:
{options}
{after-help}",
    after_help = "\
QUICK START:
  1. eval \"$(cswap shell-init zsh)\"   # add to ~/.zshrc (or bash), once
  2. cswap login                      # register the current claude login
  3. cswap login --new                # log into more accounts, inside cswap

  cswap default work        set what bare `claude` uses everywhere
  cswap activate            pick the account for THIS terminal (menu)
  cswap list                see everyone at a glance
  cswap usage               see the full picture

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

    /// Register the current claude login; --new logs into a fresh account
    #[command(
        long_about = "Register the account currently logged into ~/.claude, keyed by\n\
its email. If that email is already registered its stored tokens are\n\
refreshed. On a terminal you are offered an optional alias right away.\n\n\
--new launches claude in an empty staging profile so you can log into a\n\
DIFFERENT account from inside cswap — the live ~/.claude login is never\n\
touched; exit claude (/exit) and the account is captured automatically.\n\n\
cswap only READS ~/.claude — captures are stored under ~/.local/share/cswap.\n\n\
EXAMPLES:\n  cswap login                    # register current login\n  \
cswap login --new              # log into another account inside cswap\n  \
cswap login --new --alias work # ...and label it immediately"
    )]
    Login {
        /// Alias to attach to the account
        #[arg(long)]
        alias: Option<String>,
        /// Log into a NEW account inside cswap (staging profile; the live
        /// ~/.claude login is not touched)
        #[arg(long)]
        new: bool,
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
    /// Table of all accounts: status, email, aliases, usage
    #[command(
        long_about = "One borderless row per account — which are active/default,\n\
the email, its aliases, and the 5h/7d gates at a glance. Percentages are\n\
colored <70 green, <90 yellow, else red.\n\n\
For bars, per-model windows and reset times, use `cswap usage`.\n\n\
EXAMPLES:\n  cswap list             # with usage (one API call per account)\n  \
cswap list --quick     # instant, no network"
    )]
    List {
        /// Skip the usage API calls
        #[arg(short, long)]
        quick: bool,
    },

    /// Manage aliases: list, create, remove (interactive when args omitted)
    #[command(
        long_about = "Aliases are the labels over email identities; they resolve\n\
everywhere an account is referenced.\n\n\
EXAMPLES:\n  cswap alias list\n  cswap alias create            # pick account, type alias\n  \
cswap alias create work w     # scripted\n  cswap alias remove            # pick from a menu\n  \
cswap alias remove w"
    )]
    Alias {
        #[command(subcommand)]
        action: AliasCmd,
    },

    /// Show or swap the default account (the live ~/.claude login)
    #[command(
        long_about = "The default is not stored — it IS whoever is logged into the\n\
live ~/.claude, i.e. what a bare `claude` uses when nothing is activated.\n\n\
With no argument: report the live login and whether it's registered.\n\
With an account: SWAP the live login by copying that account's credentials\n\
into ~/.claude (the only command that writes there). If the account being\n\
displaced isn't registered its credentials would be lost, so you must type\n\
`yes` (or pass --yes) — `cswap login` it first to keep it.\n\n\
EXAMPLES:\n  cswap default             # who is live? (menu when on a tty)\n  \
cswap default work        # make work the live ~/.claude login\n  \
cswap default me@corp.com --yes"
    )]
    Default {
        /// Alias or email (interactive menu when omitted)
        key: Option<String>,
        /// Overwrite an unregistered live login without the typed confirmation
        #[arg(long)]
        yes: bool,
    },

    /// Forget an account (menu when no argument; always confirms)
    #[command(
        long_about = "Remove a registered profile: its config entry, stored tokens,\n\
and profile directory — after a confirmation (skip with --yes). The profile\n\
contains only symlinks into ~/.claude plus the account's own identity\n\
files; your real Claude data (history, settings, plugins) is never touched."
    )]
    Remove {
        /// Alias or email (interactive menu when omitted)
        key: Option<String>,
        /// Skip the confirmation prompt
        #[arg(long)]
        yes: bool,
    },

    // -------------------------------------------------------------- Session
    /// Set the account for THIS terminal (menu when no argument)
    #[command(
        long_about = "Set the active account for THIS terminal only. With no\n\
argument, an interactive menu lists every account (plus a back-to-default\n\
choice). Requires the shell integration (eval \"$(cswap shell-init zsh)\").\n\
Other terminals are unaffected; new terminals start on the default.\n\n\
EXAMPLES:\n  cswap activate            # interactive picker\n  \
cswap activate work       # by alias\n  cswap activate you@x.com  # by email\n  \
cswap activate default    # back to the default account"
    )]
    Activate {
        /// Alias or email (interactive menu when omitted)
        key: Option<String>,
        /// Emit the export line for the shell wrapper to eval
        #[arg(long, hide = true)]
        print: bool,
    },

    /// Run claude as an account: cswap run [ALIAS|EMAIL] [CLAUDE_ARGS]...
    #[command(long_about = "Run claude once as a specific account, ignoring\n\
active/default. The first argument is treated as an account only when it\n\
matches an alias or email; everything else passes to claude verbatim. With\n\
no arguments on a terminal, an interactive picker asks which account.\n\
cswap exec()s the real claude binary — signals and exit codes are native.\n\n\
The account logged into the live ~/.claude runs against ~/.claude itself\n\
(no profile) so cswap never touches its tokens.\n\n\
EXAMPLES:\n  cswap run                      # interactive picker\n  \
cswap run work -r              # resume picker, work pays\n  \
cswap run -- --model opus      # active/default account, flags pass through")]
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    // --------------------------------------------------------------- Limits
    /// Detailed per-account usage with bars and reset times
    #[command(
        long_about = "A card per account: every window (5h, 7d, per-model weekly)\n\
with a bar, percentage and reset countdown. Same source as `cswap list`,\n\
which shows only the 5h/7d numbers on one line.\n\n\
EXAMPLES:\n  cswap usage          # every account\n  \
cswap usage work     # just one"
    )]
    Usage {
        /// Alias or email (all accounts when omitted)
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
    /// List every alias and the email it points to
    List,
    /// Add an alias: cswap alias create [ACCOUNT] [ALIAS]
    Create {
        account: Option<String>,
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
        Cmd::Login { alias, new } => cmds::login::run(alias, new),
        Cmd::Activate { key, print } => cmds::activate::run(key, print),
        Cmd::List { quick } => cmds::list::run(quick),
        Cmd::Default { key, yes } => cmds::default_cmd::run(key, yes),
        Cmd::Run { args } => cmds::run::run(args),
        Cmd::Usage { key } => cmds::usage::run(key),
        Cmd::Watch { interval } => cmds::watch::run(interval),
        Cmd::ShellInit { shell } => cmds::shell_init::run(&shell),
        Cmd::Upgrade => cmds::upgrade::run(),
        Cmd::Alias { action } => match action {
            AliasCmd::List => cmds::alias::list(),
            AliasCmd::Create { account, alias } => cmds::alias::create(account, alias),
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
