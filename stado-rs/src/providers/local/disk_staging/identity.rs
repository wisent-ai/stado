//! Who the staging directory belongs to: the effective uid, the /etc/passwd
//! row model and its parser, the agent user name that the override or the
//! passwd file resolves to, and the root-only chown that hands a freshly
//! created staging directory to that user.

use super::*;

/// Effective uid through the same typed Unix API used by staging ownership.
pub(super) fn euid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

/// One /etc/passwd row (only the fields the staging repair needs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswdEntry {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
}

/// The next field of a passwd row, or why the row has none.
fn passwd_field<'a>(fields: &mut std::str::Split<'a, char>, row: &str, name: &str) -> Result<&'a str, String> {
    fields
        .next()
        .ok_or_else(|| format!("/etc/passwd row {row:?} ends before its {name} field"))
}

/// A numeric field of a passwd row, or why it is not a number.
fn passwd_id(fields: &mut std::str::Split<'_, char>, row: &str, name: &str) -> Result<u32, String> {
    let field = passwd_field(fields, row, name)?;
    field
        .parse()
        .map_err(|error| format!("/etc/passwd row {row:?} has {name} {field:?}, not a number: {error}"))
}

/// Pure /etc/passwd parser, standing in for Python's `pwd` module (nix's
/// `user` feature is not in the port's dependency set). Blank lines and the
/// `#` comments macOS ships are not rows; any other line missing a field or
/// holding a non-numeric id is refused with the row, so the agent user is
/// never resolved past a row nobody could read.
pub fn parse_passwd(text: &str) -> Result<Vec<PasswdEntry>, String> {
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|row| {
            let mut fields = row.split(':');
            let name = passwd_field(&mut fields, row, "name")?;
            if name.is_empty() {
                return Err(format!("/etc/passwd row {row:?} has an empty account name"));
            }
            passwd_field(&mut fields, row, "password")?;
            let uid = passwd_id(&mut fields, row, "uid")?;
            let gid = passwd_id(&mut fields, row, "gid")?;
            Ok(PasswdEntry {
                name: name.to_string(),
                uid,
                gid,
            })
        })
        .collect()
}

/// The passwd rows, or why /etc/passwd cannot be read or parsed: read as no
/// rows, the agent user became its bare uid and no staging directory was
/// handed over.
fn read_passwd() -> Result<Vec<PasswdEntry>, String> {
    let text = std::fs::read_to_string("/etc/passwd")
        .map_err(|error| format!("cannot read /etc/passwd: {error}"))?;
    parse_passwd(&text)
}

/// Resolve the explicitly selected staging user or the effective account.
/// An empty override or an unknown effective account is a named refusal,
/// not permission to substitute a different staging owner.
pub fn agent_user() -> Result<PasswdEntry, String> {
    let explicit = crate::config_file::variable("WISENT_STAGING_USER")?;
    let entry = if let Some(explicit) = explicit.as_deref() {
        let explicit = explicit.trim();
        if explicit.is_empty() {
            return Err("WISENT_STAGING_USER is empty; set it to the staging account name or unset it to use the effective account".to_string());
        }
        read_passwd()?.into_iter().find(|entry| entry.name == explicit)
            .ok_or_else(|| format!("WISENT_STAGING_USER names {explicit:?}, but /etc/passwd has no such account"))?
    } else {
        let uid = euid();
        read_passwd()?.into_iter().find(|entry| entry.uid == uid)
            .ok_or_else(|| format!("/etc/passwd has no account for effective UID {uid}; set WISENT_STAGING_USER to the staging account name"))?
    };
    let effective = nix::unistd::Uid::from_raw(euid());
    if !effective.is_root() && entry.uid != effective.as_raw() {
        return Err(format!(
            "effective UID {} cannot hand staging to {} (UID {}); select the effective account or run staging handover as root",
            effective.as_raw(), entry.name, entry.uid,
        ));
    }
    Ok(entry)
}

/// Python `_chown_if_root`: hand the staging dir to the agent user. Every
/// failure is said: Python's bare `except OSError: pass` left a root-owned
/// staging directory the agent user then could not write, with no word why.
pub(super) fn chown_if_root(path: &Path, entry: &PasswdEntry) -> Result<(), String> {
    if !nix::unistd::Uid::from_raw(euid()).is_root() {
        return Ok(());
    }
    let user = &entry.name;
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::metadata(path)
        .map_err(|error| format!("{} cannot be examined: {error}", path.display()))?;
    if md.uid() != entry.uid || md.gid() != entry.gid {
        std::os::unix::fs::chown(path, Some(entry.uid), Some(entry.gid))
            .map_err(|error| format!("{} cannot be handed to {user}: {error}", path.display()))?;
    }
    let after = std::fs::metadata(path)
        .map_err(|error| format!("{} ownership cannot be read back: {error}", path.display()))?;
    if after.uid() != entry.uid || after.gid() != entry.gid {
        return Err(format!(
            "{} ownership is {}:{}, expected {}:{} for {user} after staging handover",
            path.display(), after.uid(), after.gid(), entry.uid, entry.gid,
        ));
    }
    Ok(())
}
