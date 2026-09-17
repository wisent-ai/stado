//! The two argument refusals and the remote invocation they guard.

use crate::deploy::{shlex_quote, DeployError};

use super::program::{AGE_ENV, APPLY_ENV, FORCE_ENV, PRUNE_ENV, REMOTE_SCRIPT, ROOT_ENV};

/// Reject a root that is not an absolute path, so a relative argument cannot
/// resolve against whatever directory the remote shell happens to start in.
pub fn validate_root(root: &str) -> Result<(), DeployError> {
    if !root.starts_with('/') {
        return Err(DeployError(format!("cache root must be absolute: {root}")));
    }
    Ok(())
}

/// Reject an age that is not a plain digit run: it goes into `find -mtime`.
pub fn validate_days(days: &str) -> Result<(), DeployError> {
    if days.is_empty() || !days.chars().all(|c| c.is_ascii_digit()) {
        return Err(DeployError(format!("min age must be whole days: {days}")));
    }
    Ok(())
}

/// The remote invocation. Unlike the other host commands this one does not
/// escalate: build caches belong to the user that produced them, and running
/// as root would let it delete another account's files.
///
/// `prune` is the home-relative list of directories the walk must not open,
/// chosen for the target's platform by the caller; it travels as one
/// newline-separated variable so a path with spaces stays one path.
pub fn remote_command(root: &str, days: &str, apply: bool, force: bool, prune: &[&str]) -> String {
    format!(
        "/usr/bin/env {}={} {}={} {}={} {}={} {}={} /bin/sh -c {}",
        ROOT_ENV,
        shlex_quote(root),
        AGE_ENV,
        shlex_quote(days),
        APPLY_ENV,
        shlex_quote(if apply { "apply" } else { "" }),
        FORCE_ENV,
        shlex_quote(if force { "force" } else { "" }),
        PRUNE_ENV,
        shlex_quote(&prune.join("\n")),
        shlex_quote(REMOTE_SCRIPT)
    )
}
