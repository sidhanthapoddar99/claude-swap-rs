//! The few things that differ between Unix and Windows: becoming another
//! program, links, file permissions and asking whether a process is alive.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, ExitStatus};

/// Become `cmd`. On Unix the process image is replaced, so the agent gets the
/// terminal, the signals and the pid as if cswap had never been there. Windows
/// has no such call: cswap starts the agent, waits and leaves with its code.
pub fn become_program(mut cmd: Command) -> Result<std::convert::Infallible> {
    let name = cmd.get_program().to_string_lossy().into_owned();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        Err(err).with_context(|| format!("cannot run {name}"))
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().with_context(|| format!("cannot run {name}"))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// Run `cmd` in the foreground and wait. Ctrl-C goes to the child only, so
/// cancelling it brings control back here instead of ending cswap too.
pub fn run_foreground(cmd: &mut Command) -> std::io::Result<ExitStatus> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `signal` is async-signal-safe and is the only call made
        // between fork and exec; an ignored signal would otherwise be
        // inherited by the child.
        unsafe {
            cmd.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                Ok(())
            });
        }
        // SAFETY: changing the disposition of SIGINT for this process, and
        // putting the previous one back once the child has ended.
        let before = unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN) };
        let status = cmd.status();
        unsafe {
            libc::signal(libc::SIGINT, before);
        }
        status
    }
    #[cfg(not(unix))]
    {
        cmd.status()
    }
}

/// Create a link at `link` that points to `target`.
pub fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        // Windows links are typed. A target that does not exist yet is taken
        // to be a file.
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    }
}

/// Remove a link, whatever it points at.
pub fn remove_link(link: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        // A directory link is removed as a directory on Windows.
        if std::fs::remove_dir(link).is_ok() {
            return Ok(());
        }
    }
    std::fs::remove_file(link)
}

/// Restrict a file or directory to its owner. Windows profile directories are
/// already private to the user, so there is nothing to do there.
pub fn make_private(path: &Path, is_dir: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if is_dir { 0o700 } else { 0o600 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, is_dir);
        Ok(())
    }
}

/// Make a file that is about to be created readable by its owner only.
pub fn private_open(opts: &mut std::fs::OpenOptions) -> &mut std::fs::OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts
}

/// Can users other than the owner read this?
pub fn others_can_read(md: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        md.permissions().mode() & 0o077 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = md;
        false
    }
}

/// Is this file something the system would run?
pub fn is_program(md: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        md.is_file() && md.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        md.is_file()
    }
}

/// The user's home directory.
pub fn home() -> Option<std::path::PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    var("HOME")
        .or_else(|| {
            if cfg!(windows) {
                var("USERPROFILE")
            } else {
                None
            }
        })
        .map(std::path::PathBuf::from)
}

/// Ask a process to end, or with `hard` end it without asking. Does nothing
/// where there is no such signal.
pub fn terminate(pid: u32, hard: bool) {
    #[cfg(unix)]
    {
        let signal = if hard { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: a plain signal to one pid; a pid that is gone is an error
        // the caller finds out by looking.
        unsafe { libc::kill(pid as libc::pid_t, signal) };
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, hard);
    }
}

/// Is a process with this id running?
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 sends nothing; it only asks whether the pid exists.
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        // No dependency-free way to ask on Windows. `tasklist` answers.
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
            .unwrap_or(false)
    }
}

/// The name a program has on disk: `claude` on Unix, `claude.exe` or
/// `claude.cmd` on Windows.
pub fn program_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        ["exe", "cmd", "bat", "ps1"]
            .iter()
            .map(|ext| format!("{name}.{ext}"))
            .collect()
    } else {
        vec![name.to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_alive_and_a_made_up_pid_is_not() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(0x3fff_fff0));
    }

    #[cfg(unix)]
    #[test]
    fn a_foreground_command_reports_its_exit_status() {
        let status = run_foreground(Command::new("sh").args(["-c", "exit 3"])).unwrap();
        assert_eq!(status.code(), Some(3));
    }
}
