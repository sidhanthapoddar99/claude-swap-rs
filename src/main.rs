//! cswap: accounts, usage and resets for coding agents, in one place.
//!
//! - `cswap` opens the dashboard.
//! - `cswap <agent> [args…]` runs an agent on the right account. Everything
//!   after the agent's name is the agent's, so it never goes through the
//!   argument parser below.
//! - `cswap <command>` is one of the plain commands.

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use std::process::ExitCode;

mod cli;
mod fsx;
mod harness;
mod health;
mod herdr;
mod history;
mod http;
mod integration;
mod manage;
mod migrate;
mod model;
mod paths;
mod picker;
mod platform;
mod refresh;
mod sessions;
mod shell;
mod store;
mod suggest;
mod sync;
#[cfg(test)]
mod testutil;
mod text;
mod tui;
mod update;
mod view;
mod wrap;

use harness::Harness;
use text::Paint;

#[derive(Parser)]
#[command(name = "cswap", version, disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum HerdrCmd {
    /// Every installed agent with its accounts, as JSON
    Menu,
    /// Which account each herdr terminal's agent runs on
    Ls {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// The account the agent in a terminal runs on
    Info {
        /// The terminal: `t7` or its name
        terminal: String,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Move the agent in a terminal to another account, same conversation
    Switch {
        /// The terminal: `t7` or its name
        terminal: String,
        /// The account to move to: a name, an alias or `root`
        account: String,
        /// Start a new conversation when the running one cannot be told
        #[arg(long)]
        fresh: bool,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Put the cswap layout into herdr, or bring it up to date
    Install {
        /// Show the layout in herdr once it is installed
        #[arg(long)]
        show: bool,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Whether the cswap layout is installed in herdr
    Status {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the dashboard (what a bare `cswap` does)
    #[command(visible_alias = "ui")]
    Dashboard,

    /// Pick an agent and an account, then run it
    #[command(
        after_help = "Examples:\n  cswap start                 ask for the agent, then the account\n  cswap start --default       ask for the agent, start on its default account\n  cswap start codex           ask only for the account\n  cswap start -- --resume     pass --resume to the agent"
    )]
    Start {
        /// Only ask for the agent; start it on its default account
        #[arg(long)]
        default: bool,
        /// The agent, to skip the first question: claude, codex, agy or grok
        agent: Option<String>,
        /// Arguments for the agent, after `--`
        #[arg(last = true)]
        args: Vec<String>,
    },

    /// Agents in herdr terminals: which account, and switch it
    #[command(
        after_help = "Examples:\n  cswap herdr ls                  which account each terminal's agent runs on\n  cswap herdr info t7              the account the agent in t7 runs on\n  cswap herdr switch t7 work      stop it, start it on `work`, same conversation\n  cswap herdr menu                agents and accounts for a launch menu, as JSON\n  cswap herdr install --show      put the cswap layout into herdr and show it\n  cswap herdr status              whether the layout is installed\n\nA switch presses Ctrl+C until the agent leaves, then types the launch on the\nnew account at the shell behind it. It refuses in the middle of a turn."
    )]
    Herdr {
        #[command(subcommand)]
        what: HerdrCmd,
    },

    /// Which agents are running, and on which account
    Ps {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },

    /// Every account and its state
    #[command(visible_alias = "ls")]
    List {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },

    /// Limits, pace and reset times
    Usage {
        /// One account; all of them when left out
        account: Option<String>,
        /// Fetch from the vendor first
        #[arg(short, long)]
        refresh: bool,
    },

    /// Spare resets and what resets when (view only)
    Resets,

    /// Which account to use next, and why
    Suggest {
        /// One agent; all of them when left out
        agent: Option<String>,
    },

    /// Add an account and sign in
    #[command(
        after_help = "Examples:\n  cswap add claude work       a Claude Code account called work\n  cswap add codex             asks for the name\n\nThe agent's own sign-in runs next. The account is kept only if it completes,\nand only if no other account already holds that login."
    )]
    Add {
        /// claude, codex, agy or grok
        agent: String,
        /// The account's name, used as --cswap=<name>
        name: Option<String>,
    },

    /// Sign in to an account again
    Login {
        /// An alias, a key such as claude-2, or an email
        account: String,
    },

    /// Delete an account's login
    #[command(visible_alias = "rm")]
    Remove {
        /// An alias, a key such as claude-2, or an email
        account: String,
        /// Do not ask
        #[arg(short, long)]
        yes: bool,
    },

    /// Give an account another name
    Rename {
        /// A name, a key such as claude-2, or an email
        account: String,
        /// The new name
        name: String,
    },

    /// Show or set the extra names an account answers to
    #[command(
        after_help = "Examples:\n  cswap alias work              show them\n  cswap alias work w office     set them\n  cswap alias work --clear      remove them"
    )]
    Alias {
        /// A name, a key such as claude-2, or an email
        account: String,
        /// The aliases; they replace the ones it has
        aliases: Vec<String>,
        /// Remove every alias
        #[arg(long, conflicts_with = "aliases")]
        clear: bool,
    },

    /// Show or set an agent's default: where `cswap <agent>` starts
    #[command(
        alias = "default",
        after_help = "Examples:\n  cswap use claude            show it\n  cswap use claude work       always start on work\n  cswap use claude auto       let cswap pick at every launch\n  cswap use claude root       the agent's own login"
    )]
    Use {
        /// claude, codex, agy or grok
        agent: String,
        /// An account, `root` or `auto`
        account: Option<String>,
    },

    /// The agents' own logins
    #[command(alias = "defaults")]
    Root {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },

    /// Fetch usage now
    Refresh {
        /// One account; every signed-in one when left out
        account: Option<String>,
    },

    /// Bring an account's shared links up to date
    Sync {
        /// One account; all of them when left out
        account: Option<String>,
        /// Move files that hide a link to ~/.cswap/parked/ and relink
        #[arg(long)]
        repair: bool,
        /// Only report; change nothing
        #[arg(long, conflicts_with = "repair")]
        check: bool,
    },

    /// Check for problems
    Doctor,

    /// Move the profiles of cswap 0.6 into ~/.cswap
    Migrate,

    /// Print shell integration for bash, zsh, fish or powershell
    #[command(
        after_help = "Add one line to your shell's rc file:\n  bash, zsh    eval \"$(cswap shell-init zsh)\"\n  fish         cswap shell-init fish | source\n  powershell   Invoke-Expression (& cswap shell-init powershell | Out-String)\n\nAfter that, `claude`, `codex`, `agy` and `grok` run through cswap."
    )]
    ShellInit {
        /// bash, zsh, fish or powershell
        shell: String,
    },

    /// Install the latest release
    #[command(visible_alias = "upgrade")]
    Update {
        /// Only say whether a newer release exists
        #[arg(long)]
        check: bool,
    },
}

/// The top-level help, written by hand so the commands sit in groups.
fn help(p: &Paint) -> String {
    const W: usize = 26;
    let head = |s: &str| format!("{}\n", p.bold(s));
    // One row: the command padded to a fixed column, then what it does.
    let row = |cmd: &str, what: &str| format!("  {}{what}\n", p.info(&text::fit(cmd, W)));
    let mut out = String::new();

    out.push_str(&format!(
        "{} {}\nAccounts, usage and resets for your coding agents.\n\n",
        p.bold("cswap"),
        env!("CARGO_PKG_VERSION")
    ));

    out.push_str(&head("USAGE"));
    out.push_str(&row("cswap", "open the dashboard"));
    out.push_str(&row(
        "cswap <agent> [args…]",
        "run an agent on the account it starts on",
    ));
    out.push_str(&row(
        "cswap start [--default]",
        "pick an agent and an account, then run it",
    ));
    out.push_str(&row(
        "cswap <command> [options]",
        "one of the commands below",
    ));

    out.push('\n');
    out.push_str(&head("AGENTS"));
    for a in Harness::ALL {
        let note = if a.spec().experimental {
            "  (experimental)"
        } else {
            ""
        };
        out.push_str(&row(
            a.spec().bins[0],
            &format!("{}{}", a.name(), p.dim(note)),
        ));
    }
    out.push_str("\n  Everything after the agent's name goes to the agent untouched, except:\n");
    for (flag, what) in wrap::FLAGS {
        out.push_str(&row(&format!("  {flag}"), what));
    }

    let groups: [(&str, &[(&str, &str)]); 4] = [
        (
            "LOOK",
            &[
                ("list", "every account and its state"),
                ("root", "the agents' own logins"),
                ("ps", "which agents are running, on which account"),
                ("usage [account]", "weekly limits and when they reset"),
                ("resets", "spare resets and what resets when (view only)"),
                ("suggest [agent]", "which account to use next, and why"),
            ],
        ),
        (
            "ACCOUNTS",
            &[
                ("add <agent> <name>", "add an account and sign in"),
                ("login <account>", "sign in again"),
                ("rename <account> <name>", "give an account another name"),
                (
                    "alias <account> [names…]",
                    "extra names an account answers to",
                ),
                (
                    "use <agent> [account]",
                    "the agent's default: where it starts",
                ),
                ("remove <account>", "delete an account's login"),
            ],
        ),
        (
            "UPKEEP",
            &[
                ("refresh [account]", "fetch usage now"),
                (
                    "sync [account]",
                    "bring shared links up to date (--check, --repair)",
                ),
                ("doctor", "check for problems"),
                ("migrate", "move the profiles of cswap 0.6 into ~/.cswap"),
            ],
        ),
        (
            "SETUP",
            &[
                (
                    "shell-init <shell>",
                    "print shell integration: bash, zsh, fish, powershell",
                ),
                ("update", "install the latest release (--check only looks)"),
                (
                    "herdr switch <t> <acct>",
                    "move a herdr terminal's agent to another account",
                ),
                (
                    "herdr install",
                    "put the cswap layout into herdr (--show switches to it)",
                ),
            ],
        ),
    ];
    for (title, rows) in groups {
        out.push('\n');
        out.push_str(&head(title));
        for (cmd, what) in rows {
            out.push_str(&row(cmd, what));
        }
    }

    out.push('\n');
    out.push_str(&head("ACCOUNT NAMES"));
    out.push_str(&format!(
        "  Every account has a name ({}). It also answers to its aliases, its key\n  ({}), its number within the agent ({}) and its email. {} is the\n  agent's own login.\n",
        p.info("work"),
        p.info("claude-2"),
        p.info("2"),
        p.info("root")
    ));

    out.push('\n');
    out.push_str(&head("EXAMPLES"));
    out.push_str(&format!(
        "  claude --cswap=work                     {}\n",
        p.dim("with shell integration")
    ));
    out.push_str("  cswap codex --cswap=auto exec \"fix it\"\n");
    out.push_str("  cswap add claude work\n");
    out.push_str("  cswap start\n");

    out.push('\n');
    out.push_str(&format!(
        "  {}\n",
        p.dim("cswap <command> -h    more on one command")
    ));
    out.push_str(&format!(
        "  {}\n",
        p.dim("files: ~/.cswap  ·  https://github.com/sidhanthapoddar99/claude-swap-rs")
    ));
    out
}

fn dashboard() -> Result<()> {
    // The check runs beside the dashboard and is read from its cache there.
    let settings = store::Settings::load()?;
    if update::enabled(&settings) {
        std::thread::spawn(update::check_if_due);
    }
    match tui::run()? {
        Some(tui::app::Exit::Launch { key }) => {
            let (h, _) =
                store::parse_key(&key).ok_or_else(|| anyhow::anyhow!("bad account key {key}"))?;
            wrap::run(h, vec![format!("--cswap={key}")])
        }
        None => {
            if let Some(v) = update::known_newer().filter(|_| update::enabled(&settings)) {
                eprintln!(
                    "{}",
                    Paint::stderr()
                        .dim(&format!("cswap {v} is out. Install it with: cswap update"))
                );
            }
            Ok(())
        }
    }
}

fn run() -> Result<ExitCode> {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    // An agent's name first: the rest belongs to the agent, word for word.
    if let Some(h) = args.first().and_then(|a| Harness::from_id(a)) {
        wrap::run(h, args[1..].to_vec())?;
        return Ok(ExitCode::SUCCESS);
    }
    if args.is_empty() {
        dashboard()?;
        return Ok(ExitCode::SUCCESS);
    }

    let command = Cli::command().override_help(help(&Paint::stdout()));
    let cli = Cli::from_arg_matches(&command.get_matches())?;
    let Some(cmd) = cli.command else {
        dashboard()?;
        return Ok(ExitCode::SUCCESS);
    };
    // Commands whose output is read by a program, or that start an agent,
    // stay silent about updates.
    let quiet = matches!(
        cmd,
        Cmd::ShellInit { .. }
            | Cmd::Start { .. }
            | Cmd::Dashboard
            | Cmd::Update { .. }
            | Cmd::Ps { json: true }
            | Cmd::List { json: true }
            | Cmd::Root { json: true }
            | Cmd::Herdr { .. }
    );
    let mut ok = true;
    match cmd {
        Cmd::Dashboard => dashboard()?,
        Cmd::Start {
            default,
            agent,
            args,
        } => cli::start(default, agent.as_deref(), args)?,
        Cmd::Ps { json } => cli::ps(json)?,
        Cmd::Herdr { what } => match what {
            HerdrCmd::Menu => herdr::menu_cmd()?,
            HerdrCmd::Ls { json } => herdr::ls_cmd(json)?,
            HerdrCmd::Info { terminal, json } => herdr::info_cmd(&terminal, json)?,
            HerdrCmd::Switch {
                terminal,
                account,
                fresh,
                json,
            } => herdr::switch_cmd(&terminal, &account, fresh, json)?,
            HerdrCmd::Install { show, json } => integration::install_cmd(show, json)?,
            HerdrCmd::Status { json } => ok = integration::status_cmd(json)?,
        },
        Cmd::List { json } => cli::list(json)?,
        Cmd::Usage { account, refresh } => cli::usage(account.as_deref(), refresh)?,
        Cmd::Resets => cli::resets()?,
        Cmd::Suggest { agent } => cli::suggest_cmd(agent.as_deref())?,
        Cmd::Add { agent, name } => cli::add(&agent, name.as_deref())?,
        Cmd::Login { account } => cli::login(&account)?,
        Cmd::Remove { account, yes } => cli::remove(&account, yes)?,
        Cmd::Rename { account, name } => cli::rename(&account, &name)?,
        Cmd::Alias {
            account,
            aliases,
            clear,
        } => cli::alias(&account, &aliases, clear)?,
        Cmd::Use { agent, account } => cli::use_cmd(&agent, account.as_deref())?,
        Cmd::Root { json } => cli::root(json)?,
        Cmd::Refresh { account } => cli::refresh_cmd(account.as_deref())?,
        Cmd::Sync {
            account,
            repair,
            check,
        } => cli::sync_cmd(account.as_deref(), repair, check)?,
        Cmd::Doctor => ok = cli::doctor()?,
        Cmd::Migrate => migrate::run()?,
        Cmd::ShellInit { shell } => cli::shell_init(&shell)?,
        Cmd::Update { check } => update::run(check)?,
    }
    if !quiet {
        if let Ok(settings) = store::Settings::load() {
            update::nudge(&settings);
        }
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{} {e:#}", Paint::stderr().bad("cswap:"));
            ExitCode::FAILURE
        }
    }
}
