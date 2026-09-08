//! The words a recipe may carry: one check per field, shared verbatim by
//! `add` and `edit` so both accept and refuse exactly the same words.

use crate::cli::CmdError;
use crate::deploy::products;

/// `^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$` — kebab-case recipe names, so a name
/// is safe verbatim in a shell word, a JSON key and a table column.
pub(super) fn is_recipe_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    let inner_ok = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
    let edge_ok = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    match (bytes.first(), bytes.last()) {
        (Some(&first), Some(&last)) if edge_ok(first) && edge_ok(last) => {
            bytes.iter().all(|&byte| inner_ok(byte))
        }
        _ => false,
    }
}

/// One check per recipe field, so `add` — which sets every field — and
/// `edit` — which replaces the fields it was given — accept and refuse
/// exactly the same words. A checker never rewrites what it accepts: a
/// recipe stores the repo, branch, command and paths the operator typed.
pub(super) fn check_repo(repo: &str) -> Result<(), CmdError> {
    if repo.starts_with("https://") {
        return Ok(());
    }
    Err(CmdError::usage("--repo must be an https:// clone URL"))
}

pub(super) fn check_branch(branch: &str) -> Result<(), CmdError> {
    if branch.trim().is_empty() {
        return Err(CmdError::usage("--branch must name a branch"));
    }
    Ok(())
}

pub(super) fn check_command(command: &str) -> Result<(), CmdError> {
    if command.trim().is_empty() {
        return Err(CmdError::usage("--command must be a build command"));
    }
    Ok(())
}

/// Artifact paths name something the checkout left behind: relative, inside
/// it, and not empty. An absolute path or a `..` hop would upload a file the
/// build did not produce.
pub(super) fn check_artifacts(artifacts: &[String]) -> Result<(), CmdError> {
    if artifacts.iter().any(|path| {
        let path = path.trim();
        path.is_empty() || path.starts_with('/') || path.split('/').any(|part| part == "..")
    }) {
        return Err(CmdError::usage(
            "--artifact paths must be relative to the checkout, without '..'",
        ));
    }
    Ok(())
}

pub(super) fn check_interval_seconds(interval_seconds: u64) -> Result<(), CmdError> {
    if interval_seconds == 0 {
        return Err(CmdError::usage("--interval-seconds must be positive"));
    }
    Ok(())
}

/// Every `--platform` word resolved against the published platform table,
/// in the order given and without repeats: an unknown word is a usage error
/// naming the accepted ones, and asking twice for the same platform builds
/// it once.
pub(in crate::cli::builds) fn canonical_platforms(
    platforms: &[String],
) -> Result<Vec<String>, CmdError> {
    let mut canonical: Vec<String> = Vec::with_capacity(platforms.len());
    for platform in platforms {
        let word = products::managed_platform(platform.trim())
            .map_err(|error| CmdError::usage(error.to_string()))?;
        if !canonical.iter().any(|seen| seen == word) {
            canonical.push(word.to_string());
        }
    }
    if canonical.is_empty() {
        return Err(CmdError::usage(
            "--platform must name at least one platform",
        ));
    }
    Ok(canonical)
}
