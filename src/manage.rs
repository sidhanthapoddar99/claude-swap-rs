//! Creating, signing in, relabelling and removing accounts. The dashboard and
//! the command line both go through here, so the rules live in one place.

use anyhow::{bail, Context, Result};
use chrono::Utc;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::harness::Harness;
use crate::sessions::Live;
use crate::store::{self, Account, AccountMeta, HarnessSettings, Registry};
use crate::sync::{self, Report};
use crate::{fsx, paths, refresh};

/// The real binary for `h`, honouring a path set in its settings.
pub fn find_bin(h: Harness, hs: &HarnessSettings) -> Result<PathBuf> {
    h.binary(hs.binary.as_deref())
}

/// Is the real binary for `h` installed?
pub fn installed(h: Harness) -> bool {
    HarnessSettings::load(h)
        .ok()
        .is_some_and(|hs| find_bin(h, &hs).is_ok())
}

/// Why `name` cannot be an account's name or alias, if it cannot.
fn check_name(reg: &Registry, h: Harness, name: &str, except: Option<u32>) -> Result<()> {
    if name.is_empty() {
        bail!("an account needs a name");
    }
    if !store::valid_label(name) {
        bail!(
            "'{name}' cannot be a name: use letters, digits, '-', '_' or '.', \
             not only digits, and not a reserved word"
        );
    }
    if !reg.label_free(h, name, except) {
        bail!("{} already has an account called '{name}'", h.name());
    }
    Ok(())
}

/// A name to offer for an account that signs in as `email`: the part before
/// the @, with a number after it when that name is taken. Empty when the
/// email gives nothing usable.
pub fn suggest_name(reg: &Registry, h: Harness, email: &str) -> String {
    let base: String = email
        .split('@')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(24)
        .collect();
    if base.is_empty() {
        return String::new();
    }
    std::iter::once(base.clone())
        .chain((2..100).map(|n| format!("{base}-{n}")))
        .find(|name| store::valid_label(name) && reg.label_free(h, name, None))
        .unwrap_or_default()
}

/// Register a new, empty account for `h`, called `name`. It has a directory
/// and its links, and no login yet.
pub fn create(h: Harness, name: &str) -> Result<Account> {
    if !h.wrappable() {
        bail!(
            "{} cannot be pointed at another config dir, so cswap cannot hold a second account for it",
            h.name()
        );
    }
    let name = name.trim();
    fsx::private_dir(&paths::accounts_dir())?;
    let mut acc = fsx::with_lock(&paths::accounts_dir(), || {
        let reg = Registry::load()?;
        check_name(&reg, h, name, None)?;
        let id = reg.next_id(h);
        let mut acc = Account {
            harness: h,
            meta: AccountMeta {
                harness: h.id().to_string(),
                id,
                label: name.to_string(),
                created_at: Some(Utc::now()),
                ..AccountMeta::default()
            },
        };
        fsx::private_dir(&acc.dir())?;
        acc.update(|_| {})?;
        Ok(acc)
    })?;
    let hs = HarnessSettings::load(h)?;
    prepare(&mut acc, &hs)?;
    Ok(acc)
}

/// Make an account's `config_dir` ready for the agent: create it, seed it the
/// first time, and bring its links in line with the native dir. The links have
/// to exist before the agent runs, or it creates real directories that then
/// hide the shared ones.
pub fn prepare(acc: &mut Account, hs: &HarnessSettings) -> Result<Report> {
    if acc.is_native() {
        return Ok(Report::default());
    }
    let dir = acc.config_dir();
    let fresh = !dir.exists();
    fsx::private_dir(&dir)?;
    if fresh {
        acc.harness.seed(&dir)?;
    }
    if hs.sync.enabled {
        sync::sync_account(acc, hs)
    } else {
        Ok(sync::check(acc.harness, &dir, &hs.sync.exclude))
    }
}

/// Point the agent at this account: the environment, and for an agent moved
/// by a flag, that flag. Call this BEFORE adding the user's own arguments.
pub fn apply_env(cmd: &mut Command, acc: &Account) {
    let spec = acc.harness.spec();
    let var = format!("CSWAP_{}", acc.harness.id().to_uppercase());
    // Children of this session stay on the same account.
    cmd.env(&var, acc.meta.id.to_string());
    cmd.env("CSWAP_ACCOUNT", acc.key());
    if acc.is_native() {
        // Only undo a redirect cswap itself made. A config dir the user set
        // on purpose is theirs.
        if let Some(home_env) = spec.home_env {
            let ours = std::env::var_os(home_env)
                .is_some_and(|v| PathBuf::from(v).starts_with(paths::root()));
            if ours {
                cmd.env_remove(home_env);
                for (name, _) in acc.harness.launch_env() {
                    cmd.env_remove(name);
                }
            }
        }
        return;
    }
    // An agent moved by a flag gets it in front of the user's arguments.
    cmd.args(acc.harness.launch_args(&acc.config_dir()));
    if let Some(home_env) = spec.home_env {
        cmd.env(home_env, acc.config_dir());
    }
    for (name, value) in acc.harness.launch_env() {
        cmd.env(name, value);
    }
    for name in spec.scrub_env {
        cmd.env_remove(name);
    }
}

/// The command that runs the agent's own sign-in on this account.
pub fn login_command(acc: &mut Account) -> Result<Command> {
    let hs = HarnessSettings::load(acc.harness)?;
    let bin = find_bin(acc.harness, &hs)?;
    prepare(acc, &hs)?;
    let mut cmd = Command::new(bin);
    apply_env(&mut cmd, acc);
    cmd.args(acc.harness.spec().login_args);
    Ok(cmd)
}

/// After the sign-in command exits: did a login arrive? Records who it is.
pub fn finish_login(acc: &mut Account) -> Result<bool> {
    let dir = acc.config_dir();
    acc.harness.harden(&dir);
    if !acc.harness.logged_in(&dir) {
        return Ok(false);
    }
    refresh::refresh_local(acc)?;
    // A new login makes any stored "needs the user" state obsolete.
    acc.update(|m| {
        m.refresh.needs_user = false;
        m.refresh.error = None;
        m.refresh.last_attempt = None;
    })?;
    Ok(true)
}

/// Run the agent's own sign-in on this account, in the foreground. True when
/// a login was saved.
pub fn sign_in(acc: &mut Account) -> Result<bool> {
    let mut cmd = login_command(acc)?;
    crate::platform::run_foreground(&mut cmd)
        .with_context(|| format!("cannot start {}", acc.harness.spec().bins[0]))?;
    finish_login(acc)
}

/// The two lines said before an agent's sign-in takes over the terminal.
pub fn sign_in_banner(acc: &Account) -> String {
    let h = acc.harness;
    let mut text = format!(
        "cswap · signing in {} with {}'s own sign-in\n",
        acc.key(),
        h.name()
    );
    if h.spec().login_args.is_empty() {
        text.push_str(&format!(
            "  {} has no sign-in command, so this is the agent itself.\n  Sign in, then quit it to come back.\n",
            h.name()
        ));
    } else {
        text.push_str("  Ctrl-C cancels.\n");
    }
    text
}

/// Does another cswap account of the same harness already hold this login?
pub fn duplicate_of<'a>(reg: &'a Registry, acc: &Account) -> Option<&'a Account> {
    let id = acc.meta.identity.account_id.as_deref();
    let email = acc.meta.identity.email.as_deref();
    reg.of(acc.harness)
        .filter(|o| !o.is_native() && o.meta.id != acc.meta.id)
        .find(|o| {
            let same_id = id.is_some() && o.meta.identity.account_id.as_deref() == id;
            let same_email = email.is_some() && o.meta.identity.email.as_deref() == email;
            // Two workspaces of one email are different accounts, so an id on
            // both sides decides; the email is the fallback.
            match (id, o.meta.identity.account_id.as_deref()) {
                (Some(_), Some(_)) => same_id,
                _ => same_email,
            }
        })
}

/// Forget an account: its directory, credentials included. The links inside
/// are removed without being followed, so the native dir is untouched.
pub fn remove(acc: &Account, live: &Live) -> Result<()> {
    if acc.is_native() {
        bail!(
            "account 0 is {}'s own login in {}; cswap does not own it and cannot remove it",
            acc.harness.name(),
            paths::tilde(&acc.harness.native_dir())
        );
    }
    if let Some(pids) = live.get(&acc.key()).filter(|p| !p.is_empty()) {
        bail!(
            "{} has {} running session(s) (pid {}). Close them first",
            acc.key(),
            pids.len(),
            pids.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let dir = acc.dir();
    if dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("cannot remove {}", dir.display()))?;
    }
    let _ = fs::remove_file(paths::history_file(&acc.key()));
    // A bare launch that started here falls back to the agent's own login.
    let id = acc.meta.id.to_string();
    let hs = HarnessSettings::load(acc.harness)?;
    if hs.start == id {
        HarnessSettings::update(acc.harness, |s| s.start = "default".to_string())?;
    }
    Ok(())
}

/// Give an account another name. The agent's own login has none.
pub fn rename(acc: &mut Account, name: &str) -> Result<()> {
    if acc.is_native() {
        bail!(
            "{}'s own login is called default and cannot be renamed",
            acc.harness.name()
        );
    }
    let name = name.trim();
    check_name(&Registry::load()?, acc.harness, name, Some(acc.meta.id))?;
    acc.update(|m| m.label = name.to_string())
}

/// Replace the extra names an account answers to. An empty list clears them.
pub fn set_aliases(acc: &mut Account, aliases: &[String]) -> Result<()> {
    if acc.is_native() {
        bail!(
            "{}'s own login is called default and takes no alias",
            acc.harness.name()
        );
    }
    let reg = Registry::load()?;
    let mut clean: Vec<String> = Vec::new();
    for a in aliases.iter().map(|a| a.trim()).filter(|a| !a.is_empty()) {
        if a.eq_ignore_ascii_case(&acc.meta.label)
            || clean.iter().any(|c| c.eq_ignore_ascii_case(a))
        {
            continue;
        }
        check_name(&reg, acc.harness, a, Some(acc.meta.id))?;
        clean.push(a.to_string());
    }
    acc.update(|m| m.aliases = clean)
}

/// Make `sel` the account a bare launch of `h` starts on. `sel` is `default`
/// (the agent's own login), `auto`, or anything that names one account.
pub fn set_start(h: Harness, sel: &str) -> Result<String> {
    let value = match sel.trim().to_lowercase().as_str() {
        "auto" => "auto".to_string(),
        "native" | "default" | "0" => "default".to_string(),
        other => {
            let reg = Registry::load()?;
            let acc = reg.find(h, other)?;
            if acc.is_native() {
                "default".to_string()
            } else {
                acc.meta.id.to_string()
            }
        }
    };
    HarnessSettings::update(h, |s| s.start = value.clone())?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Sandbox;

    #[test]
    fn create_numbers_accounts_and_links_them() {
        let sb = Sandbox::new();
        let native = sb.native(Harness::Claude);
        fs::write(native.join("settings.json"), "{}").unwrap();

        let a = create(Harness::Claude, "work").unwrap();
        let b = create(Harness::Claude, " home ").unwrap();
        assert_eq!(b.meta.label, "home");
        assert_eq!((a.key(), b.key()), ("claude-1".into(), "claude-2".into()));
        assert!(a.dir().join("settings.json").is_file());
        assert!(fs::symlink_metadata(a.config_dir().join("settings.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(a.meta.sync.linked, 1);

        // The name is taken now, within this agent only.
        assert!(create(Harness::Claude, "work").is_err());
        assert!(create(Harness::Claude, "WORK").is_err());
        sb.native(Harness::Codex);
        assert!(create(Harness::Codex, "work").is_ok());
    }

    #[test]
    fn an_account_needs_a_name_that_is_not_a_number_or_a_reserved_word() {
        let sb = Sandbox::new();
        sb.native(Harness::Claude);
        for bad in ["", "  ", "3", "default", "native", "auto", "has space"] {
            let err = create(Harness::Claude, bad);
            assert!(err.is_err(), "'{bad}' was taken as a name");
        }
        assert!(Registry::load().unwrap().profiles().next().is_none());
    }

    #[test]
    fn renaming_and_aliases_keep_every_name_of_an_agent_apart() {
        let sb = Sandbox::new();
        sb.native(Harness::Claude);
        let mut work = create(Harness::Claude, "work").unwrap();
        let mut home = create(Harness::Claude, "home").unwrap();

        set_aliases(&mut work, &["w".into(), "office".into(), "W".into()]).unwrap();
        assert_eq!(work.meta.aliases, ["w", "office"]);
        // Another account can take neither a name nor an alias in use.
        assert!(rename(&mut home, "office").is_err());
        assert!(set_aliases(&mut home, &["work".into()]).is_err());
        // An account's own names are not in its own way.
        rename(&mut work, "office").unwrap();
        assert_eq!(work.meta.label, "office");
        // A name cannot be taken away, only replaced.
        assert!(rename(&mut work, "").is_err());
        set_aliases(&mut work, &[]).unwrap();
        assert!(work.meta.aliases.is_empty());

        let reg = Registry::load().unwrap();
        assert_eq!(
            reg.find(Harness::Claude, "office").unwrap().key(),
            work.key()
        );
        let mut own = reg.find(Harness::Claude, "default").unwrap().clone();
        assert!(rename(&mut own, "mine").is_err());
        assert!(set_aliases(&mut own, &["mine".into()]).is_err());
    }

    #[test]
    fn a_name_is_offered_from_the_email_and_numbered_when_taken() {
        let sb = Sandbox::new();
        sb.native(Harness::Claude);
        let reg = Registry::load().unwrap();
        assert_eq!(
            suggest_name(&reg, Harness::Claude, "dev@corp.example"),
            "dev"
        );
        assert_eq!(suggest_name(&reg, Harness::Claude, ""), "");
        create(Harness::Claude, "dev").unwrap();
        let reg = Registry::load().unwrap();
        assert_eq!(
            suggest_name(&reg, Harness::Claude, "dev@corp.example"),
            "dev-2"
        );
        // A name that reads as a number or a reserved word is not offered bare.
        assert_eq!(suggest_name(&reg, Harness::Claude, "auto@x.io"), "auto-2");
    }

    #[test]
    fn only_another_account_counts_as_holding_the_same_login() {
        let sb = Sandbox::new();
        sb.native(Harness::Claude);
        let mut work = create(Harness::Claude, "work").unwrap();
        let mut home = create(Harness::Claude, "home").unwrap();
        work.update(|m| m.identity.email = Some("me@x.io".into()))
            .unwrap();
        home.update(|m| m.identity.email = Some("me@x.io".into()))
            .unwrap();
        let mut reg = Registry::load().unwrap();
        // The agent's default may hold the same login as an account.
        reg.get_mut("claude-0").unwrap().meta.identity.email = Some("me@x.io".into());
        let own = reg.get("claude-0").unwrap();
        assert_eq!(duplicate_of(&reg, own).unwrap().key(), work.key());
        assert_eq!(duplicate_of(&reg, &home).unwrap().key(), work.key());
        assert_eq!(duplicate_of(&reg, &work).unwrap().key(), home.key());
        home.update(|m| m.identity.email = Some("other@x.io".into()))
            .unwrap();
        let reg = Registry::load().unwrap();
        assert!(duplicate_of(&reg, &work).is_none());
    }

    #[test]
    fn remove_keeps_the_native_dir_and_resets_a_default_that_pointed_here() {
        let sb = Sandbox::new();
        let native = sb.native(Harness::Claude);
        fs::create_dir_all(native.join("projects")).unwrap();
        fs::write(native.join("projects/keep.jsonl"), "x").unwrap();
        let a = create(Harness::Claude, "work").unwrap();
        set_start(Harness::Claude, "1").unwrap();

        remove(&a, &Live::new()).unwrap();
        assert!(!a.dir().exists());
        assert!(native.join("projects/keep.jsonl").is_file());
        assert_eq!(
            HarnessSettings::load(Harness::Claude).unwrap().start,
            "default"
        );
    }

    #[test]
    fn a_late_write_does_not_bring_a_removed_account_back() {
        let sb = Sandbox::new();
        sb.native(Harness::Claude);
        let a = create(Harness::Claude, "gone").unwrap();
        // A fetch that started before the removal still holds the account.
        let mut in_flight = a.clone();
        remove(&a, &Live::new()).unwrap();
        assert!(in_flight
            .update(|m| m.refresh.error = Some("late".into()))
            .is_err());
        assert!(!a.dir().exists());
        assert!(Registry::load().unwrap().get(&a.key()).is_none());
    }

    #[test]
    fn remove_refuses_the_default_login_and_live_accounts() {
        let sb = Sandbox::new();
        sb.native(Harness::Claude);
        let a = create(Harness::Claude, "work").unwrap();
        let mut live = Live::new();
        live.insert(a.key(), vec![42]);
        assert!(remove(&a, &live).is_err());
        let reg = Registry::load().unwrap();
        let native = reg.find(Harness::Claude, "default").unwrap();
        assert!(remove(native, &Live::new()).is_err());
    }

    #[test]
    fn a_bare_launch_can_start_on_a_word_a_number_or_a_name() {
        let sb = Sandbox::new();
        sb.native(Harness::Claude);
        create(Harness::Claude, "work").unwrap();
        assert_eq!(set_start(Harness::Claude, "work").unwrap(), "1");
        assert_eq!(set_start(Harness::Claude, "AUTO").unwrap(), "auto");
        assert_eq!(set_start(Harness::Claude, "0").unwrap(), "default");
        assert_eq!(set_start(Harness::Claude, "1").unwrap(), "1");
        assert_eq!(set_start(Harness::Claude, "default").unwrap(), "default");
        assert!(set_start(Harness::Claude, "nobody").is_err());
    }
}
