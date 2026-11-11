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

// --------------------------------------------------------------- conversations

/// The conversation a running agent is in, as far as disk can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Talk {
    /// Nothing has been said yet: a fresh start loses nothing.
    Fresh,
    /// A conversation, and the arguments that open it again on any account.
    Open { id: String, resume: Vec<String> },
    /// cswap cannot tell. Never guessed: resuming the wrong conversation
    /// looks right and is not.
    Unknown,
}

impl Talk {
    pub fn word(&self) -> &'static str {
        match self {
            Talk::Fresh => "fresh",
            Talk::Open { .. } => "open",
            Talk::Unknown => "unknown",
        }
    }

    fn open(id: String, flag: &str) -> Talk {
        Talk::Open {
            resume: vec![flag.to_string(), id.clone()],
            id,
        }
    }
}

fn proc_dir() -> std::path::PathBuf {
    std::env::var_os("CSWAP_PROC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/proc"))
}

/// The files a process holds open. `None` when that cannot be read, which is
/// every system but Linux.
fn open_files(pid: u32) -> Option<Vec<std::path::PathBuf>> {
    let entries = std::fs::read_dir(proc_dir().join(pid.to_string()).join("fd")).ok()?;
    Some(
        entries
            .flatten()
            .filter_map(|e| std::fs::read_link(e.path()).ok())
            .collect(),
    )
}

pub fn command_line(pid: u32) -> Vec<String> {
    std::fs::read(proc_dir().join(pid.to_string()).join("cmdline"))
        .map(|raw| {
            raw.split(|b| *b == 0)
                .map(|a| String::from_utf8_lossy(a).to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The parent of a process, where the system lets cswap read it.
pub fn parent(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(proc_dir().join(pid.to_string()).join("stat")).ok()?;
    // `pid (name) state ppid ...`, and the name may hold spaces and brackets.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// A session id as the agents write them: hex digits and dashes.
fn is_id(text: &str) -> bool {
    text.len() >= 16 && text.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Which conversation the agent `pid` of `h` is in. `config_dir` is the dir
/// it runs on; the files read here are shared by every account of the agent.
pub fn talk(h: crate::harness::Harness, pid: u32, config_dir: &std::path::Path) -> Talk {
    use crate::harness::Harness;
    match h {
        Harness::Claude => claude_talk(pid, config_dir),
        Harness::Codex => codex_talk(pid),
        Harness::Grok => grok_talk(pid, config_dir),
        Harness::Agy => agy_talk(pid),
    }
}

/// Claude Code notes each running session in `sessions/<pid>.json`. The
/// conversation can be resumed once its transcript exists.
fn claude_talk(pid: u32, config_dir: &std::path::Path) -> Talk {
    let id = std::fs::read(config_dir.join("sessions").join(format!("{pid}.json")))
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("sessionId")?.as_str().map(str::to_string))
        .filter(|id| is_id(id));
    let Some(id) = id else {
        return Talk::Unknown;
    };
    let Ok(projects) = std::fs::read_dir(config_dir.join("projects")) else {
        return Talk::Fresh;
    };
    let file = format!("{id}.jsonl");
    if projects.flatten().any(|p| p.path().join(&file).is_file()) {
        Talk::open(id, "--resume")
    } else {
        Talk::Fresh
    }
}

/// Codex keeps the transcript of its conversation open: a `rollout-*.jsonl`
/// whose first line names the session. A helper thread has a transcript of its
/// own, with a parent, and is not the conversation.
fn codex_talk(pid: u32) -> Talk {
    use std::io::BufRead;

    let Some(files) = open_files(pid) else {
        return Talk::Unknown;
    };
    let mut found: Vec<(std::time::SystemTime, String)> = Vec::new();
    for path in files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if !(name.starts_with("rollout-") && name.ends_with(".jsonl")) {
            continue;
        }
        let Ok(file) = std::fs::File::open(&path) else {
            return Talk::Unknown;
        };
        let mut first = String::new();
        if std::io::BufReader::new(file).read_line(&mut first).is_err() {
            return Talk::Unknown;
        }
        let Ok(meta) = serde_json::from_str::<serde_json::Value>(&first) else {
            return Talk::Unknown;
        };
        let payload = &meta["payload"];
        if !payload["parent_thread_id"].is_null() || payload["source"].is_object() {
            continue;
        }
        let Some(id) = payload["id"].as_str().filter(|id| is_id(id)) else {
            return Talk::Unknown;
        };
        let changed = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        found.push((changed, id.to_string()));
    }
    found.sort();
    if let Some((_, id)) = found.pop() {
        return Talk::open(id, "resume");
    }
    // Resumed, and nothing written since: the command line still says which.
    let args = command_line(pid);
    match args.iter().position(|a| a == "resume") {
        Some(at) => match args.get(at + 1).filter(|id| is_id(id)) {
            Some(id) => Talk::open(id.clone(), "resume"),
            None => Talk::Unknown,
        },
        None => Talk::Fresh,
    }
}

/// Grok lists its running sessions in `active_sessions.json`, from the start.
fn grok_talk(pid: u32, config_dir: &std::path::Path) -> Talk {
    std::fs::read(config_dir.join("active_sessions.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Vec<serde_json::Value>>(&raw).ok())
        .and_then(|all| {
            all.iter()
                .find(|s| s["pid"].as_u64() == Some(u64::from(pid)))
                .and_then(|s| s["session_id"].as_str().map(str::to_string))
        })
        .filter(|id| is_id(id))
        .map(|id| Talk::open(id, "--resume"))
        .unwrap_or(Talk::Unknown)
}

/// agy holds its conversation's files open under `antigravity-cli`. One id
/// among them is the conversation; none or several is not an answer.
fn agy_talk(pid: u32) -> Talk {
    let Some(files) = open_files(pid) else {
        return Talk::Unknown;
    };
    let mut ids: Vec<String> = files
        .iter()
        .filter(|p| {
            let parent = p.parent().and_then(|d| d.file_name()).unwrap_or_default();
            parent == "conversations" || parent == "presence"
        })
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .filter(|id| is_id(id))
        .collect();
    ids.sort();
    ids.dedup();
    match ids.as_slice() {
        [id] => Talk::open(id.clone(), "--conversation"),
        _ => Talk::Unknown,
    }
}

#[cfg(test)]
mod talk_tests {
    use super::*;
    use crate::harness::Harness;
    use crate::testutil::Sandbox;
    use std::fs;

    const ID: &str = "32efd4c2-3347-4240-a60a-61b639bc1ece";
    const OTHER: &str = "01a1bf9a-76b0-7b43-bb3d-7bdb7ad98004";

    /// A process in the sandbox's `/proc` that holds `files` open.
    #[cfg(unix)]
    fn process(sb: &Sandbox, pid: u32, args: &[&str], files: &[&std::path::Path]) {
        let dir = sb.root().join("proc").join(pid.to_string());
        fs::create_dir_all(dir.join("fd")).unwrap();
        fs::write(dir.join("cmdline"), args.join("\0")).unwrap();
        for (n, file) in files.iter().enumerate() {
            std::os::unix::fs::symlink(file, dir.join("fd").join(n.to_string())).unwrap();
        }
    }

    #[test]
    fn claude_resumes_only_a_conversation_that_has_a_transcript() {
        let sb = Sandbox::new();
        let dir = sb.native(Harness::Claude);
        assert_eq!(talk(Harness::Claude, 7, &dir), Talk::Unknown);

        fs::create_dir_all(dir.join("sessions")).unwrap();
        fs::write(
            dir.join("sessions/7.json"),
            format!(r#"{{"pid":7,"sessionId":"{ID}"}}"#),
        )
        .unwrap();
        assert_eq!(talk(Harness::Claude, 7, &dir), Talk::Fresh);

        fs::create_dir_all(dir.join("projects/-home-me-code")).unwrap();
        fs::write(dir.join(format!("projects/-home-me-code/{ID}.jsonl")), "{}").unwrap();
        assert_eq!(
            talk(Harness::Claude, 7, &dir),
            Talk::Open {
                id: ID.to_string(),
                resume: vec!["--resume".to_string(), ID.to_string()],
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn codex_is_read_from_the_transcript_it_holds_open_not_its_helpers() {
        let sb = Sandbox::new();
        let dir = sb.native(Harness::Codex);
        let main = dir.join(format!("rollout-2026-10-31T04-33-55-{ID}.jsonl"));
        let helper = dir.join(format!("rollout-2026-11-09T14-48-53-{OTHER}.jsonl"));
        fs::write(
            &main,
            format!(r#"{{"type":"session_meta","payload":{{"id":"{ID}","source":"cli"}}}}"#),
        )
        .unwrap();
        fs::write(
            &helper,
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{OTHER}","parent_thread_id":"{ID}","source":{{"subagent":{{"other":"guardian"}}}}}}}}"#
            ),
        )
        .unwrap();

        // Nothing said yet: no transcript, no `resume` on the command line.
        process(&sb, 11, &["codex", "--no-daemon"], &[]);
        assert_eq!(talk(Harness::Codex, 11, &dir), Talk::Fresh);

        process(&sb, 12, &["codex"], &[&helper, &main]);
        assert_eq!(
            talk(Harness::Codex, 12, &dir),
            Talk::Open {
                id: ID.to_string(),
                resume: vec!["resume".to_string(), ID.to_string()],
            }
        );

        // Resumed and idle: the command line still names it.
        process(&sb, 13, &["codex", "--no-daemon", "resume", OTHER], &[]);
        assert!(matches!(talk(Harness::Codex, 13, &dir), Talk::Open { id, .. } if id == OTHER));

        // A process that cannot be read is not called fresh.
        assert_eq!(talk(Harness::Codex, 99, &dir), Talk::Unknown);
    }

    #[cfg(unix)]
    #[test]
    fn grok_and_agy_name_their_session_or_stay_unknown() {
        let sb = Sandbox::new();
        let grok = sb.native(Harness::Grok);
        assert_eq!(talk(Harness::Grok, 21, &grok), Talk::Unknown);
        fs::write(
            grok.join("active_sessions.json"),
            format!(r#"[{{"session_id":"{ID}","pid":21,"cwd":"/x"}}]"#),
        )
        .unwrap();
        assert!(matches!(talk(Harness::Grok, 21, &grok), Talk::Open { id, .. } if id == ID));
        assert_eq!(talk(Harness::Grok, 22, &grok), Talk::Unknown);

        let agy = sb.native(Harness::Agy).join("antigravity-cli");
        fs::create_dir_all(agy.join("conversations")).unwrap();
        fs::create_dir_all(agy.join("presence")).unwrap();
        let db = agy.join(format!("conversations/{ID}.db"));
        let lock = agy.join(format!("presence/{ID}.lock"));
        let second = agy.join(format!("conversations/{OTHER}.db"));
        for f in [&db, &lock, &second] {
            fs::write(f, "").unwrap();
        }
        process(&sb, 31, &["agy"], &[&db, &lock]);
        assert_eq!(
            talk(Harness::Agy, 31, &agy),
            Talk::Open {
                id: ID.to_string(),
                resume: vec!["--conversation".to_string(), ID.to_string()],
            }
        );
        process(&sb, 32, &["agy"], &[&db, &second]);
        assert_eq!(talk(Harness::Agy, 32, &agy), Talk::Unknown);
        process(&sb, 33, &["agy"], &[]);
        assert_eq!(talk(Harness::Agy, 33, &agy), Talk::Unknown);
    }
}
