//! `cswap agent-info`: what a coding agent needs to know to use cswap.
//!
//! It is written for the agent, not the person: short, exact commands, and
//! what to leave alone. Everything it names is a real command of this binary;
//! the end-to-end tests run each one it lists.

/// The guide. `{version}` is filled in when it is printed.
const TEXT: &str = r#"# cswap {version}: a guide for agents

cswap keeps several logins ("accounts") for each coding agent (claude, codex,
grok, agy) and starts an agent on one of them. Accounts of one agent share
their settings, history and conversations; only the login differs. You may be
running on one now.

## Where you are

  echo "$CSWAP_ACCOUNT"        the account you run on, such as claude-2.
                               Empty: cswap did not start you (the root login).
  cswap ps --json              every running agent and its account
  cswap herdr ls --json        the same, per herdr terminal

## Starting another agent

Start it as `cswap <agent>`, never by its bare name. A bare `claude` reaches
cswap only where the user turned PATH shims on (`cswap shims` says). Anywhere
else it skips cswap and runs on the root login, whatever account you are on.

  cswap claude "<task>"                    the default the user set
  cswap claude --cswap=work "<task>"       the account named work
  cswap codex --cswap=auto "<task>"        the account with the most room now
  cswap claude --cswap=work,side "<task>"  the first of these that is free

In herdr, put cswap in front of the agent's own start command:

  hc run --bg --name w1 cswap claude "<task>"
  hc run --bg --name w2 cswap codex --no-daemon "<task>"

With no --cswap flag the account is, first match wins:
  1. CSWAP_CLAUDE, CSWAP_CODEX, CSWAP_GROK or CSWAP_AGY in the environment.
     cswap sets it for the agent it starts, so a program you run yourself,
     in your own shell tool, stays on your account.
  2. The agent's default, which the user set.

A terminal herdr starts (`hc run`) gets the login environment, not yours.
So a sub-agent started there is on its agent's default, whatever account you
are on and whatever kind of agent it is. To put it on your own account, name
it: --cswap="$CSWAP_ACCOUNT" works for an agent of your own kind.

Everything else on the command line goes to the agent untouched.

## Choosing an account

  cswap list --json            every account: agent, name, state, plan, usage
  cswap suggest [agent]        which account to use next, and why
  cswap usage [account]        limits and when they reset, from what is stored

These read what cswap has stored and ask no vendor. `cswap usage -r` and
`cswap refresh` do ask; do not run them in a loop.

## Moving or restarting a running agent (herdr)

  cswap herdr info <terminal> --json     its account, and whether it can move
  cswap herdr switch <terminal> <account>
  cswap herdr restart <terminal>         the same account, to apply an update

A switch stops the agent and starts it again on the other account in the same
conversation, in the same terminal. A restart does that on the account it is
on. Both refuse in the middle of a turn, and while sub-agents of that agent
are working: a report sent to it meanwhile is lost. Do not pass --force
unless the user says so. Never switch or restart your own terminal: it stops
the agent in it, which is you.

## Account names

An account answers to its name (work), its aliases, its key (claude-2), its
number within the agent (2) and its email. `root` is the agent's own login.

## Leave these to the user

  add, login, remove, rename, alias, use, migrate, update, sync --repair,
  herdr install, shims on, shims off, uninstall
      They change the user's logins, defaults or setup. Ask first.
  Files under ~/.cswap/accounts/*/config_dir hold tokens. Do not read, print
  or copy them.
  cswap shows spare resets and cannot spend one. Do not try to either.

More: cswap -h, and cswap <command> -h for one command.
"#;

pub fn text() -> String {
    TEXT.replace("{version}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    /// A terminal an agent reads in is often 80 cells wide.
    #[test]
    fn the_guide_fits_a_narrow_terminal() {
        for line in super::text().lines() {
            assert!(line.chars().count() <= 80, "too long: {line}");
        }
    }
}
