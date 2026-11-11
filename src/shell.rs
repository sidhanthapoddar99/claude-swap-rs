//! Shell integration: the shells on this machine, the agents each one wraps,
//! and the one line in a shell's rc file that loads the wrappers.
//!
//! A wrapper is a shell function named after an agent that runs
//! `cswap <agent>`. `cswap shell-init <shell>` prints one for every agent the
//! shell wraps, and one line in the rc file evaluates that when the shell
//! starts. So the rc file is written once. After that, wrapping or unwrapping
//! an agent only changes cswap's own settings, and the next shell picks it up.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::harness::Harness;
use crate::store::{HarnessSettings, Settings};
use crate::{fsx, paths};

/// The shells cswap can wrap agents in.
pub const KNOWN: [&str; 4] = ["zsh", "bash", "fish", "powershell"];

/// The name cswap files a shell under.
pub fn canon(shell: &str) -> Option<&'static str> {
    match shell {
        "zsh" => Some("zsh"),
        "bash" | "sh" => Some("bash"),
        "fish" => Some("fish"),
        "powershell" | "pwsh" => Some("powershell"),
        _ => None,
    }
}

/// The shell the person logs in to, from `$SHELL`. Empty when it is not set.
pub fn current() -> String {
    let shell = std::env::var("SHELL").unwrap_or_default();
    shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim_end_matches(".exe")
        .to_string()
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The file a shell reads when it starts, where the cswap line goes. `None`
/// for a shell whose file cswap cannot name for sure.
pub fn rc_file(shell: &str) -> Option<PathBuf> {
    let home = paths::home();
    match canon(shell)? {
        "zsh" => Some(env_dir("ZDOTDIR").unwrap_or(home).join(".zshrc")),
        "bash" => Some(home.join(".bashrc")),
        "fish" => Some(
            env_dir("XDG_CONFIG_HOME")
                .unwrap_or_else(|| home.join(".config"))
                .join("fish")
                .join("config.fish"),
        ),
        // PowerShell's profile moves with its version and the host program.
        _ => None,
    }
}

/// The line that loads the wrappers in `shell`.
pub fn line(shell: &str) -> String {
    match canon(shell) {
        Some("fish") => "cswap shell-init fish | source".to_string(),
        Some("powershell") => {
            "Invoke-Expression (& cswap shell-init powershell | Out-String)".to_string()
        }
        Some("zsh") => "eval \"$(cswap shell-init zsh)\"".to_string(),
        _ => "eval \"$(cswap shell-init bash)\"".to_string(),
    }
}

/// The same line as cswap writes it into an rc file: it does nothing when
/// cswap is not installed, so the shell never starts with an error.
fn guarded_line(shell: &str) -> String {
    match canon(shell) {
        Some("fish") => format!("type -q cswap; and {}", line(shell)),
        _ => format!("command -v cswap >/dev/null 2>&1 && {}", line(shell)),
    }
}

/// Does the shell's rc file load cswap? `None` when cswap cannot tell.
pub fn loaded(shell: &str) -> Option<bool> {
    let text = fs::read_to_string(rc_file(shell)?).unwrap_or_default();
    Some(
        text.lines()
            .any(|l| !l.trim_start().starts_with('#') && l.contains("cswap shell-init")),
    )
}

/// Is the shell's program on this machine?
pub fn installed(shell: &str) -> bool {
    let names: &[&str] = match canon(shell) {
        Some("powershell") => &["pwsh", "powershell"],
        Some("zsh") => &["zsh"],
        Some("bash") => &["bash"],
        Some("fish") => &["fish"],
        _ => return false,
    };
    names.iter().any(|n| fsx::which(n, None).is_some())
}

/// The agents `shell` wraps. A shell with no list of its own wraps every
/// agent `was_on` allows: the switch each agent had before wrapping was
/// chosen per shell.
pub fn chosen(settings: &Settings, shell: &str, was_on: impl Fn(Harness) -> bool) -> Vec<Harness> {
    let name = canon(shell).unwrap_or(shell);
    match settings.shells.get(name) {
        Some(ids) => Harness::ALL
            .into_iter()
            .filter(|h| ids.iter().any(|id| id == h.id()))
            .collect(),
        None => Harness::ALL.into_iter().filter(|h| was_on(*h)).collect(),
    }
}

/// One shell, as the Settings page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub name: &'static str,
    /// The shell the person logs in to.
    pub current: bool,
    pub rc: Option<PathBuf>,
    /// Does the rc file load cswap? `None` when cswap cannot tell.
    pub loaded: Option<bool>,
    /// The agents its list holds, whether or not the rc file loads them.
    pub chosen: Vec<Harness>,
}

impl Status {
    /// Does `h` run through cswap in this shell? Only when the shell's list
    /// holds it and the rc file loads the wrappers.
    pub fn wraps(&self, h: Harness) -> bool {
        self.loaded != Some(false) && self.chosen.contains(&h)
    }
}

/// Every shell on this machine, and the login shell even when its program is
/// not found.
pub fn all(settings: &Settings, hs: &HashMap<Harness, HarnessSettings>) -> Vec<Status> {
    let mine = current();
    let mine = canon(&mine);
    KNOWN
        .into_iter()
        .filter(|name| Some(*name) == mine || installed(name))
        .map(|name| Status {
            name,
            current: Some(name) == mine,
            rc: rc_file(name),
            loaded: loaded(name),
            chosen: chosen(settings, name, |h| hs.get(&h).is_none_or(|s| s.wrapped)),
        })
        .collect()
}

/// What saving a shell's list did besides storing it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Saved {
    /// The rc file, when the cswap line was added to it just now.
    pub added_to: Option<PathBuf>,
    /// The line to add by hand, when cswap cannot add it itself.
    pub by_hand: Option<String>,
}

/// Store the agents `shell` wraps, and make sure its rc file loads cswap when
/// it wraps any. An rc file that already loads cswap is left alone, and so is
/// one whose shell wraps nothing: the line then prints no wrapper at all.
pub fn set_chosen(shell: &str, agents: &[Harness]) -> Result<Saved> {
    let name = canon(shell).with_context(|| {
        format!("'{shell}' is not a shell cswap knows. Use bash, zsh, fish or powershell")
    })?;
    let ids: Vec<String> = agents.iter().map(|h| h.id().to_string()).collect();
    Settings::update(|s| {
        s.shells.insert(name.to_string(), ids);
    })?;
    let mut saved = Saved::default();
    if agents.is_empty() {
        return Ok(saved);
    }
    match rc_file(name) {
        None => saved.by_hand = Some(line(name)),
        Some(rc) if loaded(name) != Some(true) => {
            add_line(&rc, name).with_context(|| {
                format!(
                    "the list is saved, but {} could not be written. Add this line to it by hand: {}",
                    paths::tilde(&rc),
                    line(name)
                )
            })?;
            saved.added_to = Some(rc);
        }
        Some(_) => {}
    }
    Ok(saved)
}

/// Append the cswap line to an rc file. It is added to the end and nothing
/// else is touched; a file reached through a link stays a link.
fn add_line(rc: &Path, shell: &str) -> Result<()> {
    if let Some(dir) = rc.parent() {
        fs::create_dir_all(dir)?;
    }
    let before = match fs::read(rc) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let mut text = String::new();
    if !before.is_empty() {
        if !before.ends_with(b"\n") {
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(
        "# cswap: run the agents through cswap. Choose which in `cswap`, Settings > Shell.\n",
    );
    text.push_str(&guarded_line(shell));
    text.push('\n');
    let mut file = fs::OpenOptions::new().create(true).append(true).open(rc)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

/// What `cswap shell-init <shell>` prints: a function for each of `agents`.
pub fn init_text(shell: &str, agents: &[Harness]) -> Result<String> {
    let bins: Vec<(&str, &str)> = agents
        .iter()
        .flat_map(|h| h.spec().bins.iter().map(move |b| (*b, h.id())))
        .collect();
    let mut out = String::from(
        "# cswap shell integration. Each agent runs through cswap, which picks the account.\n",
    );
    match shell {
        "bash" | "zsh" | "sh" => {
            for (bin, id) in &bins {
                // An alias of the same name would win over the function.
                out.push_str(&format!(
                    "unalias {bin} 2>/dev/null\n{bin}() {{ command cswap {id} \"$@\"; }}\n"
                ));
            }
        }
        "fish" => {
            for (bin, id) in &bins {
                out.push_str(&format!(
                    "function {bin} --description 'cswap: {bin}'\n    command cswap {id} $argv\nend\n"
                ));
            }
        }
        "powershell" | "pwsh" => {
            for (bin, id) in &bins {
                out.push_str(&format!("function {bin} {{ cswap.exe {id} @args }}\n"));
            }
        }
        other => {
            bail!("'{other}' is not a shell cswap knows. Use bash, zsh, fish or powershell")
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Sandbox;

    #[test]
    fn a_shell_with_no_list_wraps_every_agent_that_was_switched_on() {
        let _sb = Sandbox::new();
        let s = Settings::default();
        assert_eq!(chosen(&s, "zsh", |_| true), Harness::ALL.to_vec());
        assert_eq!(
            chosen(&s, "zsh", |h| h != Harness::Codex),
            vec![Harness::Claude, Harness::Agy, Harness::Grok]
        );
    }

    #[test]
    fn saving_a_list_adds_the_line_once_and_leaves_the_rest_of_the_file() {
        let _sb = Sandbox::new();
        let rc = rc_file("zsh").unwrap();
        fs::write(&rc, "export EDITOR=vi").unwrap();
        assert_eq!(loaded("zsh"), Some(false));

        let saved = set_chosen("zsh", &[Harness::Claude, Harness::Codex]).unwrap();
        assert_eq!(saved.added_to.as_deref(), Some(rc.as_path()));
        let text = fs::read_to_string(&rc).unwrap();
        assert!(text.starts_with("export EDITOR=vi\n\n# cswap"), "{text}");
        assert!(
            text.ends_with("eval \"$(cswap shell-init zsh)\"\n"),
            "{text}"
        );
        assert_eq!(loaded("zsh"), Some(true));

        // A second save changes the list and not the file.
        let saved = set_chosen("zsh", &[Harness::Grok]).unwrap();
        assert_eq!(saved, Saved::default());
        assert_eq!(fs::read_to_string(&rc).unwrap(), text);
        let s = Settings::load().unwrap();
        assert_eq!(chosen(&s, "zsh", |_| true), vec![Harness::Grok]);
        // Another shell still has no list of its own.
        assert_eq!(chosen(&s, "bash", |_| true), Harness::ALL.to_vec());
    }

    #[test]
    fn a_shell_that_wraps_nothing_gets_no_line() {
        let _sb = Sandbox::new();
        let rc = rc_file("bash").unwrap();
        let saved = set_chosen("bash", &[]).unwrap();
        assert_eq!(saved, Saved::default());
        assert!(!rc.exists());
        assert!(chosen(&Settings::load().unwrap(), "bash", |_| true).is_empty());
    }

    #[test]
    fn a_shell_cswap_cannot_write_for_gets_the_line_to_add_by_hand() {
        let _sb = Sandbox::new();
        let saved = set_chosen("powershell", &[Harness::Claude]).unwrap();
        assert!(saved.added_to.is_none());
        assert!(saved.by_hand.unwrap().contains("shell-init powershell"));
        assert!(set_chosen("tcsh", &[Harness::Claude]).is_err());
    }

    #[test]
    fn a_line_that_is_commented_out_does_not_count() {
        let _sb = Sandbox::new();
        let rc = rc_file("fish").unwrap();
        fs::create_dir_all(rc.parent().unwrap()).unwrap();
        fs::write(&rc, "# cswap shell-init fish | source\n").unwrap();
        assert_eq!(loaded("fish"), Some(false));
        set_chosen("fish", &[Harness::Claude]).unwrap();
        let text = fs::read_to_string(&rc).unwrap();
        assert!(
            text.ends_with("type -q cswap; and cswap shell-init fish | source\n"),
            "{text}"
        );
        assert_eq!(loaded("fish"), Some(true));
    }

    #[test]
    fn init_prints_a_function_for_each_agent_given_and_no_other() {
        let text = init_text("zsh", &[Harness::Codex]).unwrap();
        assert!(text.contains("codex() { command cswap codex \"$@\"; }"));
        assert!(!text.contains("claude()"));
        assert!(init_text("fish", &[Harness::Claude])
            .unwrap()
            .contains("function claude"));
        assert!(init_text("tcsh", &[]).is_err());
    }
}
