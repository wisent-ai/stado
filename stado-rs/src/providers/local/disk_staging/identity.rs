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

fn read_passwd() -> Vec<PasswdEntry> {
    std::fs::read_to_string("/etc/passwd")
        .map(|text| parse_passwd(&text))
        .unwrap_or_default()
}

/// Python `_agent_user`: WISENT_STAGING_USER override, else the passwd
/// name for the euid, else the numeric euid (Python's KeyError branch).
pub fn agent_user() -> String {
    if let Ok(explicit) = std::env::var("WISENT_STAGING_USER") {
        let explicit = explicit.trim();
        if !explicit.is_empty() {
            return explicit.to_string();
        }
    }
    let uid = euid();
    read_passwd()
        .into_iter()
        .find(|entry| entry.uid == uid)
        .map(|entry| entry.name)
        .unwrap_or_else(|| uid.to_string())
}

/// Python `_chown_if_root`: hand the staging dir to the agent user.
pub(super) fn chown_if_root(path: &Path, user: &str) {
    if euid() != 0 {
        return;
    }
    let Some(entry) = read_passwd().into_iter().find(|entry| entry.name == user) else {
        return;
    };
    use std::os::unix::fs::MetadataExt;
    let Ok(md) = std::fs::metadata(path) else {
        return;
    };
    if md.uid() != entry.uid {
        if let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) {
            // SAFETY: c_path is a valid NUL-terminated path; errors are
            // deliberately ignored (Python's bare `except OSError: pass`).
            unsafe { nix::libc::chown(c_path.as_ptr(), entry.uid, entry.gid) };
        }
    }
}
