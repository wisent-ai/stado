//! Who the staging directory belongs to: the effective uid, the /etc/passwd
//! row model and its parser, the agent user name that the override or the
//! passwd file resolves to, and the root-only chown that hands a freshly
//! created staging directory to that user.

use super::*;

/// Effective uid via libc (nix's typed wrappers need features outside the
/// port's allowed set).
pub(super) fn euid() -> u32 {
    // SAFETY: geteuid is always successful and async-signal-safe.
    unsafe { nix::libc::geteuid() }
}

/// One /etc/passwd row (only the fields the staging repair needs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswdEntry {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
}

/// Pure /etc/passwd parser, standing in for Python's `pwd` module (nix's
/// `user` feature is not in the port's dependency set).
pub fn parse_passwd(text: &str) -> Vec<PasswdEntry> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            fields.next()?; // password placeholder
            let uid = fields.next()?.parse().ok()?;
            let gid = fields.next()?.parse().ok()?;
            Some(PasswdEntry {
                name: name.to_string(),
                uid,
                gid,
            })
        })
        .collect()
}

/// The passwd rows, or why /etc/passwd cannot be read: read as no rows, the
/// agent user became its bare uid and no staging directory was handed over.
fn read_passwd() -> Result<Vec<PasswdEntry>, String> {
    std::fs::read_to_string("/etc/passwd")
        .map(|text| parse_passwd(&text))
        .map_err(|error| format!("cannot read /etc/passwd: {error}"))
}

/// Python `_agent_user`: WISENT_STAGING_USER override, else the passwd
/// name for the euid, else the numeric euid (Python's KeyError branch). An
/// override that is not text, or a passwd file that cannot be read, is the
/// error.
pub fn agent_user() -> Result<String, String> {
    if let Some(explicit) = crate::config_file::variable("WISENT_STAGING_USER")? {
        let explicit = explicit.trim();
        if !explicit.is_empty() {
            return Ok(explicit.to_string());
        }
    }
    let uid = euid();
    match read_passwd()?.into_iter().find(|entry| entry.uid == uid) {
        Some(entry) => Ok(entry.name),
        None => Ok(uid.to_string()),
    }
}

/// Python `_chown_if_root`: hand the staging dir to the agent user. Every
/// failure is said: Python's bare `except OSError: pass` left a root-owned
/// staging directory the agent user then could not write, with no word why.
pub(super) fn chown_if_root(path: &Path, user: &str) -> Result<(), String> {
    if euid() != 0 {
        return Ok(());
    }
    let Some(entry) = read_passwd()?.into_iter().find(|entry| entry.name == user) else {
        return Err(format!(
            "{} cannot be handed to {user}: /etc/passwd has no such user",
            path.display()
        ));
    };
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::metadata(path)
        .map_err(|error| format!("{} cannot be examined: {error}", path.display()))?;
    if md.uid() != entry.uid {
        std::os::unix::fs::chown(path, Some(entry.uid), Some(entry.gid))
            .map_err(|error| format!("{} cannot be handed to {user}: {error}", path.display()))?;
    }
    Ok(())
}
