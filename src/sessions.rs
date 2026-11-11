//! Which accounts have an agent running right now.
//!
//! Linux reads `/proc`: a process counts when its name is one of the harness's
//! binaries, and its environment says which config dir it was started on. That
//! finds every session, however it was started.
//!
//! Other systems cannot read another process's environment, so there cswap
//! notes each launch it makes in `~/.cswap/run/<pid>` and reports the ones
//! whose process is still alive. A session started without cswap is not seen
//! there.

use std::collections::HashMap;

use crate::store::Registry;

/// Account key -> pids of the agents running on it.
pub type Live = HashMap<String, Vec<u32>>;

#[cfg(target_os = "linux")]
pub fn scan(reg: &Registry) -> Live {
    use crate::harness::Harness;
    use std::fs;
    use std::path::PathBuf;

    let mut live = Live::new();
    // `CSWAP_PROC` moves it, which is what the tests use.
    let proc_dir = std::env::var_os("CSWAP_PROC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/proc"));
    let Ok(entries) = fs::read_dir(proc_dir) else {
        return live;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_string_lossy().parse::<u32>().ok() else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let Ok(comm) = fs::read_to_string(entry.path().join("comm")) else {
            continue;
        };
        let comm = comm.trim();
        let Some(h) = Harness::ALL
            .into_iter()
            .find(|h| h.spec().bins.contains(&comm))
        else {
            continue;
        };
        // Unreadable environ means another user's process: not ours to count.
        let Ok(environ) = fs::read(entry.path().join("environ")) else {
            continue;
        };
        let var = |name: &str| -> Option<PathBuf> {
            let prefix = format!("{name}=");
            environ
                .split(|b| *b == 0)
                .find(|v| v.starts_with(prefix.as_bytes()))
                .map(|v| PathBuf::from(String::from_utf8_lossy(&v[prefix.len()..]).to_string()))
        };
        // An agent moved by a flag carries its dir on the command line.
        let from_flag = h.spec().home_flag.and_then(|flag| {
            let cmdline = fs::read(entry.path().join("cmdline")).ok()?;
            cmdline
                .split(|b| *b == 0)
                .find(|a| a.starts_with(flag.as_bytes()))
                .map(|a| PathBuf::from(String::from_utf8_lossy(&a[flag.len()..]).to_string()))
        });
        let dir = from_flag.or_else(|| h.spec().home_env.and_then(var));
        let key = match dir {
            Some(dir) => reg
                .of(h)
                .find(|a| !a.is_native() && a.config_dir() == dir)
                .map(|a| a.key()),
            None => reg.of(h).find(|a| a.is_native()).map(|a| a.key()),
        };
        if let Some(key) = key {
            live.entry(key).or_default().push(pid);
        }
    }
    for pids in live.values_mut() {
        pids.sort_unstable();
    }
    live
}

#[cfg(not(target_os = "linux"))]
pub fn scan(reg: &Registry) -> Live {
    let mut live = Live::new();
    let Ok(entries) = std::fs::read_dir(run_dir()) else {
        return live;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_string_lossy().parse::<u32>().ok() else {
            continue;
        };
        if !crate::platform::pid_alive(pid) {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        let Ok(key) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let key = key.trim();
        if reg.get(key).is_some() {
            live.entry(key.to_string()).or_default().push(pid);
        }
    }
    for pids in live.values_mut() {
        pids.sort_unstable();
    }
    live
}

#[cfg(not(target_os = "linux"))]
fn run_dir() -> std::path::PathBuf {
    crate::paths::root().join("run")
}

/// Note that this process is about to become an agent on `key`. Does nothing
/// on Linux, where `/proc` already tells. Best effort: a launch never fails
/// because the note could not be written.
pub fn register(key: &str) {
    #[cfg(not(target_os = "linux"))]
    {
        let path = run_dir().join(std::process::id().to_string());
        let _ = crate::fsx::write_private(&path, key.as_bytes());
    }
    #[cfg(target_os = "linux")]
    {
        let _ = key;
    }
}

/// What can be said about one running agent: where it runs and since when.
#[derive(Debug, Clone, Default)]
pub struct Details {
    pub cwd: Option<std::path::PathBuf>,
    pub started: Option<chrono::DateTime<chrono::Utc>>,
}

pub fn details(pid: u32) -> Details {
    #[cfg(target_os = "linux")]
    {
        let dir = std::path::PathBuf::from(format!("/proc/{pid}"));
        Details {
            cwd: std::fs::read_link(dir.join("cwd")).ok(),
            // The directory appears when the process does.
            started: std::fs::metadata(&dir)
                .and_then(|m| m.modified())
                .ok()
                .map(chrono::DateTime::<chrono::Utc>::from),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let started = std::fs::metadata(run_dir().join(pid.to_string()))
            .and_then(|m| m.modified())
            .ok()
            .map(chrono::DateTime::<chrono::Utc>::from);
        Details { cwd: None, started }
    }
}
