mod cmds;
mod config;
mod oauth;
mod paths;
mod profile;
mod update_check;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cswap",
    version,
    about = "Fast multi-account switcher for Claude Code",
    long_about = "Fast multi-account switcher for Claude Code.\n\n\
Named accounts, per-terminal activation, parallel sessions, shared history.\n\
cswap never writes into ~/.claude — all its state lives in ~/.config/cswap\n\
and ~/.local/share/cswap.",
    arg_required_else_help = true,
    after_help = "\
QUICK START:
  1. eval \"$(cswap shell-init zsh)\"     # add to ~/.zshrc (or bash) once
  2. claude /login                        # log into an account
  3. cswap login                          # capture it under a name
  4. repeat 2-3 per account, then:
  cswap default work        what bare `claude` uses everywhere
  cswap activate personal   what `claude` uses in THIS terminal only
  claude -r                 just works — same shared history on every account

Run `cswap help <command>` for details and examples of each command."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Register the account currently logged into ~/.claude (or refresh it)
    #[command(
        long_about = "Register the account currently logged into ~/.claude.\n\n\
If the live email is already registered, its stored tokens are refreshed\n\
(relogin). Otherwise you are prompted for a name and the account is added;\n\
the first account becomes the default.\n\n\
cswap only READS ~/.claude — the capture is stored under ~/.local/share/cswap.\n\n\
EXAMPLES:\n  claude /login && cswap login          # interactive name prompt\n  \
cswap login --name work               # scripted"
    )]
    Login {
        /// Account name (prompted interactively when omitted)
        #[arg(long)]
        name: Option<String>,
    },

    /// Set the account for THIS terminal (no name = back to default)
    #[command(long_about = "Set the active account for THIS terminal only.\n\n\
Requires the shell integration (eval \"$(cswap shell-init zsh)\" in your rc):\n\
a child process cannot set env vars in its parent shell, so the shell\n\
function evals the export line this command prints. Other terminals are\n\
unaffected; new terminals start on the default account.\n\n\
EXAMPLES:\n  cswap activate dev     # this shell now runs claude as 'dev'\n  \
cswap activate         # back to the default account")]
    Activate {
        name: Option<String>,
        /// Emit the export line for the shell wrapper to eval
        #[arg(long, hide = true)]
        print: bool,
    },

    /// List accounts with usage, default (d) and active (*) markers
    #[command(long_about = "List all accounts: name, email, usage windows.\n\n\
Markers: * = active in this shell ($CSWAP_ACTIVE), d = default.\n\
Usage shows every window that gates the account: the 5-hour and 7-day\n\
limits plus any per-model weekly limits, with reset countdowns.\n\n\
EXAMPLES:\n  cswap list             # with usage (one API call per account)\n  \
cswap list --quick     # instant, no network")]
    List {
        /// Skip the usage API calls
        #[arg(short, long)]
        quick: bool,
    },

    /// Show or set the default account (accepts name or email)
    #[command(
        long_about = "Show or set the default account — what a bare `claude`\n\
uses in any terminal that hasn't run `cswap activate`.\n\n\
EXAMPLES:\n  cswap default                # show current\n  \
cswap default work           # set by name\n  cswap default me@corp.com    # set by email"
    )]
    Default { name: Option<String> },

    /// Run claude as an account: cswap run [NAME] [CLAUDE_ARGS]...
    #[command(long_about = "Run claude once as a specific account, ignoring\n\
active/default. The first argument is treated as an account name only when it\n\
matches one; everything else passes to claude verbatim. cswap exec()s the\n\
real claude binary — signals, exit codes, and interactivity are native.\n\n\
The account logged into the live ~/.claude runs against ~/.claude itself\n\
(no profile) so cswap never touches its tokens.\n\n\
EXAMPLES:\n  cswap run work                 # interactive claude as 'work'\n  \
cswap run work -r              # resume picker, work pays\n  \
cswap run -- --model opus      # active/default account, flags pass through")]
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Live usage dashboard (redraws every INTERVAL seconds)
    #[command(
        long_about = "Full-screen usage table for every account, redrawn on an\n\
interval (default 300s, minimum 60 — the usage API budgets ~20-30\n\
requests/hour per account token, so don't go much lower).\n\n\
EXAMPLES:\n  cswap watch\n  cswap watch -i 120"
    )]
    Watch {
        #[arg(short, long, default_value_t = 300)]
        interval: u64,
    },

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

    /// Self-update from GitHub Releases
    #[command(
        long_about = "Download the latest release for this platform, verify its\n\
SHA-256 checksum, and atomically replace this binary. `cswap list` also\n\
nudges (at most once per 24h) when a newer version exists; set\n\
CSWAP_NO_UPDATE_CHECK=1 to disable that check."
    )]
    Upgrade,

    /// Forget an account (config + stored tokens + profile dir)
    #[command(
        long_about = "Remove an account: its config entry, stored credentials,\n\
and profile directory. The profile contains only symlinks into ~/.claude\n\
plus the account's own identity files — your real Claude data (history,\n\
settings, plugins) is never touched. If the removed account was the\n\
default, the first remaining account becomes default."
    )]
    Remove { name: String },

    /// Internal: what the claude() shell wrapper calls
    #[command(name = "_claude", hide = true)]
    ClaudeShim {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Login { name } => cmds::login::run(name),
        Cmd::Activate { name, print } => cmds::activate::run(name, print),
        Cmd::List { quick } => cmds::list::run(quick),
        Cmd::Default { name } => cmds::default_cmd::run(name),
        Cmd::Run { args } => cmds::run::run(args),
        Cmd::Watch { interval } => cmds::watch::run(interval),
        Cmd::ShellInit { shell } => cmds::shell_init::run(&shell),
        Cmd::Upgrade => cmds::upgrade::run(),
        Cmd::Remove { name } => cmds::remove::run(name),
        Cmd::ClaudeShim { args } => cmds::run::shim(args),
    };
    if let Err(e) = result {
        eprintln!("cswap: {e:#}");
        std::process::exit(1);
    }
}
