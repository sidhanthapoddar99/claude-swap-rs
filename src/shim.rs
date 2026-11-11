//! PATH shims: an agent started by any program goes through cswap.
//!
//! The shell wrappers only work where a shell reads its rc file. A program
//! that starts `claude` itself, such as herdr running `hc run claude …`,
//! finds the real binary on `PATH` and never meets cswap. A shim closes that
//! gap: a link named after the agent, in a folder of cswap's own that comes
//! first on `PATH`, pointing at cswap. cswap sees the name it was called by
//! and runs that agent on the account it picks.
//!
//! It is off until asked for, and everything it does is undone by
//! `cswap shims off`:
//!
//! - the links live in `~/.cswap/shims`, nowhere else;
//! - the `PATH` line goes at the end of the login files, two lines that are
//!   removed again exactly as they were written;
//! - a link to a cswap that is gone is skipped by every `PATH` search, so the
//!   real agent still starts when cswap is deleted by hand.
//!
//! The real binary is found by skipping every `PATH` entry that is cswap
//! itself (`fsx::which`), so a shim never runs a shim.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::harness::Harness;
use crate::store::Settings;
use crate::{fsx, paths, shell};

/// Shims are links, and `PATH` is searched the way this module expects, on
/// Unix only.
pub fn supported() -> bool {
    cfg!(unix)
}

/// The folder of links. It is cswap's own, so nothing else is ever in it.
pub fn dir() -> PathBuf {
    paths::root().join("shims")
}

/// The agents that have a shim, as the settings hold them.
pub fn chosen(settings: &Settings) -> Vec<Harness> {
    if !supported() {
        return Vec::new();
    }
    Harness::ALL
        .into_iter()
        .filter(|h| settings.shims.iter().any(|id| id == h.id()))
        .collect()
}

/// The agent whose name cswap was started under, when it was started through
/// a shim: `claude` for a link called `claude`. `None` for `cswap` itself.
pub fn invoked_as() -> Option<Harness> {
    let arg0 = std::env::args_os().next()?;
    let name = Path::new(&arg0).file_name()?.to_string_lossy().to_string();
    let name = name.trim_end_matches(".exe");
    Harness::ALL
        .into_iter()
        .find(|h| h.spec().bins.contains(&name))
}

/// This binary, with every link in its path resolved.
fn me() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| fs::canonicalize(p).ok())
}

/// Does `link` lead to this cswap?
fn ours(link: &Path) -> bool {
    let target = fs::canonicalize(link).ok();
    target.is_some() && target == me()
}

/// Where one agent's shim stands in the environment this process runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// The link is there and it is the first of that name on `PATH`.
    Active,
    /// The link is missing, or leads somewhere that is not this cswap.
    Broken,
    /// The link is fine, but another program of that name comes first on
    /// `PATH` here, so a bare launch skips cswap.
    Shadowed(PathBuf),
    /// The link is fine and the folder is not on `PATH` here at all, and no
    /// other program of that name is either.
    OffPath,
}

pub fn state(h: Harness) -> State {
    let bin = h.spec().bins[0];
    let link = dir().join(bin);
    if !ours(&link) {
        return State::Broken;
    }
    match fsx::which(bin, None) {
        Some(first) if first == link || ours(&first) => State::Active,
        Some(first) => State::Shadowed(first),
        None => State::OffPath,
    }
}

/// One agent as herdr finds it: the first program of its name on herdr's
/// `PATH`, if any, and whether that program is this cswap.
pub type Seen = (Harness, Option<PathBuf>, bool);

/// What herdr itself would start for a bare `claude`: the first program of
/// that name on the `PATH` herdr's core runs with, which is the one it had
/// when it started and not this process's. `None` when herdr cannot be asked.
pub fn seen_by_herdr(agents: &[Harness]) -> Option<Vec<Seen>> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    struct Asked {
        at: Instant,
        agents: Vec<Harness>,
        seen: Option<Vec<Seen>>,
    }
    // The dashboard checks its health at every reload; herdr is asked at most
    // this often.
    const KEEP: Duration = Duration::from_secs(20);
    static LAST: Mutex<Option<Asked>> = Mutex::new(None);

    if agents.is_empty() {
        return None;
    }
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(asked) = last.as_ref() {
        if asked.at.elapsed() < KEEP && asked.agents == agents {
            return asked.seen.clone();
        }
    }
    let mut args = vec![
        "exec",
        "--timeout",
        "5",
        "sh",
        "-c",
        // One line per name: the path, or an empty line.
        r#"for b in "$@"; do command -v "$b" || echo; done"#,
        "sh",
    ];
    args.extend(agents.iter().map(|h| h.spec().bins[0]));
    let seen = crate::herdr::hc(&args).ok().and_then(|out| {
        let lines: Vec<&str> = out.lines().collect();
        (lines.len() == agents.len()).then(|| {
            agents
                .iter()
                .zip(lines)
                .map(|(h, line)| {
                    let path = Some(line.trim())
                        .filter(|l| l.starts_with('/'))
                        .map(PathBuf::from);
                    let is_ours = path.as_deref().is_some_and(ours);
                    (*h, path, is_ours)
                })
                .collect()
        })
    });
    *last = Some(Asked {
        at: Instant::now(),
        agents: agents.to_vec(),
        seen: seen.clone(),
    });
    seen
}

/// Make the folder hold a link for each of `agents` and for nothing else.
#[cfg(unix)]
fn sync_links(agents: &[Harness]) -> Result<()> {
    let dir = dir();
    let me = me().context("cannot find the running cswap binary")?;
    if !agents.is_empty() {
        fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    for h in Harness::ALL {
        for bin in h.spec().bins {
            let link = dir.join(bin);
            let wanted = agents.contains(&h);
            if wanted && ours(&link) {
                continue;
            }
            // Only a link is ever removed: the folder holds nothing else, and
            // if someone put a real file there it is theirs.
            if fs::symlink_metadata(&link).is_ok_and(|md| md.file_type().is_symlink()) {
                fs::remove_file(&link)
                    .with_context(|| format!("cannot remove {}", link.display()))?;
            }
            if wanted {
                std::os::unix::fs::symlink(&me, &link)
                    .with_context(|| format!("cannot create {}", link.display()))?;
            }
        }
    }
    if agents.is_empty() {
        // Gone when empty; left alone when something else is in it.
        let _ = fs::remove_dir(&dir);
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_links(_agents: &[Harness]) -> Result<()> {
    anyhow::bail!("PATH shims are for Linux and macOS. On Windows, use the shell integration")
}

/// The folder as a login file names it: under `$HOME` where it can be.
fn dir_for_shell() -> String {
    let dir = dir();
    match dir.strip_prefix(paths::home()) {
        Ok(rest) => format!("$HOME/{}", rest.display()),
        Err(_) => dir.display().to_string(),
    }
}

const COMMENT: &str =
    "# cswap: agents started by other programs go through cswap. Undo with: cswap shims off";

/// The line that puts the folder first on `PATH`, for bash, zsh and sh. It
/// does nothing when the folder is first already, so reading the file twice
/// does not grow `PATH`.
pub fn path_line() -> String {
    let d = dir_for_shell();
    format!("case \"$PATH\" in \"{d}:\"*) ;; *) export PATH=\"{d}:$PATH\" ;; esac")
}

/// The same for fish, which has no `case` of that shape.
fn fish_line() -> String {
    let d = dir_for_shell();
    format!("set -gx PATH \"{d}\" (string match -v -- \"{d}\" $PATH)")
}

/// What `cswap shell-init <shell>` adds while a shim is on: the folder goes
/// first again after everything the rc file put in front of it.
pub fn init_text(shell: &str, settings: &Settings) -> String {
    if !supported() || chosen(settings).is_empty() {
        return String::new();
    }
    match shell::canon(shell) {
        Some("fish") => format!("{}\n", fish_line()),
        Some("bash") | Some("zsh") => format!("{}\n", path_line()),
        _ => String::new(),
    }
}

/// The files a login reads, where the `PATH` line goes. A desktop login and
/// a login shell do not read the same one, so every file that is there gets
/// the line, and the login shell's own file is made when it is missing.
pub fn login_files() -> Vec<PathBuf> {
    let home = paths::home();
    let zdot = std::env::var_os("ZDOTDIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.clone());
    let zsh = shell::canon(&shell::current()) == Some("zsh");
    let mut files = Vec::new();
    let zprofile = zdot.join(".zprofile");
    if zsh || zprofile.is_file() {
        files.push(zprofile);
    }
    for name in [".bash_profile", ".bash_login"] {
        let file = home.join(name);
        if file.is_file() {
            files.push(file);
        }
    }
    // Read by sh, by bash without a file of its own, and by most desktop
    // logins. It is made when no login file exists at all.
    let profile = home.join(".profile");
    if profile.is_file() || files.is_empty() {
        files.push(profile);
    }
    files
}

fn has_line(text: &str) -> bool {
    let line = path_line();
    text.lines().any(|l| l.trim() == line)
}

/// Add the two lines to the end of `file`. True when they were added; false
/// when the file had the line already.
fn add_line(file: &Path) -> Result<bool> {
    let before = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", file.display())),
    };
    if has_line(&before) {
        return Ok(false);
    }
    let mut text = before.clone();
    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(COMMENT);
    text.push('\n');
    text.push_str(&path_line());
    text.push('\n');
    write_through_links(file, &text)?;
    Ok(true)
}

/// Take the two lines out of `file` again, and the blank line cswap put in
/// front of them. True when something was removed.
fn remove_line(file: &Path) -> Result<bool> {
    let Ok(before) = fs::read_to_string(file) else {
        return Ok(false);
    };
    if !has_line(&before) {
        return Ok(false);
    }
    let line = path_line();
    let mut kept: Vec<&str> = Vec::new();
    for l in before.lines() {
        if l.trim() == line {
            if kept.last().is_some_and(|k| k.trim() == COMMENT) {
                kept.pop();
                if kept.last().is_some_and(|k| k.trim().is_empty()) {
                    kept.pop();
                }
            }
            continue;
        }
        kept.push(l);
    }
    let mut text = kept.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    write_through_links(file, &text)?;
    Ok(true)
}

/// Write a login file in place. A dotfile is often a link into a dotfiles
/// repository, and it must stay one, so the target is written, not replaced.
fn write_through_links(file: &Path, text: &str) -> Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(file, text).with_context(|| format!("cannot write {}", file.display()))
}

/// What turning shims on or off did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Saved {
    pub agents: Vec<Harness>,
    /// Login files the `PATH` line was added to just now.
    pub added_to: Vec<PathBuf>,
    /// Login files the line was taken out of, or that were removed because
    /// cswap had made them and nothing else was in them.
    pub removed_from: Vec<PathBuf>,
}

/// Give `agents` a shim and nothing else one. An empty list turns the
/// feature off and undoes everything: the links, the folder, the `PATH`
/// lines, and a login file cswap made itself.
pub fn set(agents: &[Harness]) -> Result<Saved> {
    let mut saved = Saved {
        agents: agents.to_vec(),
        ..Saved::default()
    };
    sync_links(agents)?;
    let before = Settings::load()?;
    let mut made = before.shim_files_made.clone();
    if agents.is_empty() {
        // Every file that could hold the line, not only today's list: the
        // login shell may have changed since the line was written.
        let mut files = login_files();
        for f in &made {
            if !files.contains(f) {
                files.push(f.clone());
            }
        }
        for file in files {
            if remove_line(&file)? {
                let empty = fs::read_to_string(&file).is_ok_and(|t| t.trim().is_empty());
                if empty && made.contains(&file) {
                    let _ = fs::remove_file(&file);
                }
                saved.removed_from.push(file);
            }
        }
        made.clear();
    } else {
        for file in login_files() {
            let existed = file.is_file();
            if add_line(&file)? {
                if !existed && !made.contains(&file) {
                    made.push(file.clone());
                }
                saved.added_to.push(file);
            }
        }
    }
    Settings::update(|s| {
        s.shims = agents.iter().map(|h| h.id().to_string()).collect();
        s.shim_files_made = made;
    })?;
    Ok(saved)
}

/// How many times in a row cswap has been started by the very program it
/// started, read from the mark the last launch left. A wrapper script that
/// runs `claude` from `PATH` would otherwise go round forever once `claude`
/// is a shim. A real chain of agents starting agents never comes back this
/// fast, so only a loop reaches the limit.
pub fn guard_depth(now_ms: u128) -> u32 {
    let mark = std::env::var(GUARD).unwrap_or_default();
    let Some((depth, at)) = mark.split_once(':') else {
        return 0;
    };
    match (depth.parse::<u32>(), at.parse::<u128>()) {
        (Ok(depth), Ok(at)) if now_ms.saturating_sub(at) <= GUARD_WINDOW_MS => depth,
        _ => 0,
    }
}

/// The variable a launch leaves for the next one: `<depth>:<unix ms>`.
pub const GUARD: &str = "CSWAP_GUARD";
/// A launch that follows the last one within this long counts as a step of
/// the same chain.
const GUARD_WINDOW_MS: u128 = 2_000;
/// More steps than this is a loop.
pub const GUARD_LIMIT: u32 = 4;

pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testutil::Sandbox;

    #[test]
    fn the_path_line_is_added_once_and_removed_exactly() {
        let _sb = Sandbox::new();
        let file = paths::home().join(".profile");
        fs::write(&file, "export EDITOR=vi").unwrap();

        assert!(add_line(&file).unwrap());
        assert!(!add_line(&file).unwrap(), "added twice");
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("export EDITOR=vi\n\n# cswap:"), "{text}");
        assert!(text.contains("\"$HOME/.cswap/shims:\"*"), "{text}");
        assert_eq!(text.matches("shims").count(), 3, "{text}");

        // What the user wrote after it stays, and so does what came before.
        fs::write(&file, format!("{text}export PAGER=less\n")).unwrap();
        assert!(remove_line(&file).unwrap());
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "export EDITOR=vi\nexport PAGER=less\n"
        );
        assert!(!remove_line(&file).unwrap());
    }

    #[test]
    fn the_line_puts_the_folder_first_and_only_once() {
        let _sb = Sandbox::new();
        let home = paths::home();
        let script = format!(
            "PATH=/usr/bin:/bin\n{line}\n{line}\necho \"$PATH\"\n",
            line = path_line()
        );
        let out = std::process::Command::new("sh")
            .args(["-c", &script])
            .env("HOME", &home)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            format!("{}/.cswap/shims:/usr/bin:/bin", home.display())
        );
    }

    #[test]
    fn a_launch_that_comes_straight_back_counts_up_and_a_later_one_does_not() {
        let _sb = Sandbox::new();
        std::env::remove_var(GUARD);
        assert_eq!(guard_depth(10_000), 0);
        std::env::set_var(GUARD, "3:10000");
        assert_eq!(guard_depth(10_500), 3);
        // The agent ran for a while and then started another: a new chain.
        assert_eq!(guard_depth(20_000), 0);
        std::env::set_var(GUARD, "nonsense");
        assert_eq!(guard_depth(10_000), 0);
        std::env::remove_var(GUARD);
    }

    #[test]
    fn the_login_shells_own_file_gets_the_line_and_so_does_one_that_exists() {
        let _sb = Sandbox::new();
        let home = paths::home();
        // The sandbox's shell is zsh, and no login file exists yet.
        assert_eq!(login_files(), vec![home.join(".zprofile")]);
        fs::write(home.join(".profile"), "").unwrap();
        assert_eq!(
            login_files(),
            vec![home.join(".zprofile"), home.join(".profile")]
        );
        std::env::set_var("SHELL", "/bin/bash");
        assert_eq!(login_files(), vec![home.join(".profile")]);
        fs::write(home.join(".bash_profile"), "").unwrap();
        assert_eq!(
            login_files(),
            vec![home.join(".bash_profile"), home.join(".profile")]
        );
    }
}
