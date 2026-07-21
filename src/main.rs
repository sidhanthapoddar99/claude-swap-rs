mod cmds;
mod config;
mod oauth;
mod paths;
mod profile;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cswap",
    version,
    about = "Fast multi-account switcher for Claude Code",
    after_help = "Setup: add  eval \"$(cswap shell-init zsh)\"  to your shell rc, \
then `cswap login` per account."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Register the account currently logged into ~/.claude (or refresh it)
    Login {
        /// Account name (prompted interactively when omitted)
        #[arg(long)]
        name: Option<String>,
    },
    /// Set the account for THIS terminal (needs shell-init; no name = back to default)
    Activate {
        name: Option<String>,
        /// Emit the export line for the shell wrapper to eval
        #[arg(long, hide = true)]
        print: bool,
    },
    /// List accounts with usage, default and active markers
    List {
        /// Skip the usage API calls
        #[arg(short, long)]
        quick: bool,
    },
    /// Show or set the default account (name or email)
    Default { name: Option<String> },
    /// Run claude as an account: cswap run [NAME] [CLAUDE_ARGS]...
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Live usage dashboard (redraws every INTERVAL seconds)
    Watch {
        #[arg(short, long, default_value_t = 300)]
        interval: u64,
    },
    /// Print shell integration (bash|zsh) — eval it from your rc file
    ShellInit { shell: String },
    /// Forget an account (config + stored tokens + profile dir)
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
        Cmd::Remove { name } => cmds::remove::run(name),
        Cmd::ClaudeShim { args } => cmds::run::shim(args),
    };
    if let Err(e) = result {
        eprintln!("cswap: {e:#}");
        std::process::exit(1);
    }
}
