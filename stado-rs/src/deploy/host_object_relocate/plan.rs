//! What an operator may name, and the pass they named it for.

use crate::deploy::DeployError;

/// A key prefix an operator may name, or the reason it is refused.
///
/// The prefixes are spliced into the remote program inside a double-quoted
/// word, so what has to be excluded is anything the shell would still read
/// there — `"`, `$`, `` ` `` and `\` — plus the traversal a store path may
/// never contain. An absolute prefix is refused too: these are keys under a
/// namespace, and one starting with `/` would address the filesystem root.
pub fn validate_prefix(label: &str, prefix: &str) -> Result<(), DeployError> {
    if prefix.starts_with('/') {
        return Err(DeployError(format!(
            "{label} must be a key prefix inside the namespace, not an absolute path: {prefix}"
        )));
    }
    if prefix.split('/').any(|segment| segment == "..") {
        return Err(DeployError(format!(
            "{label} may not contain a `..` segment: {prefix}"
        )));
    }
    if let Some(bad) = prefix.chars().find(|character| {
        matches!(character, '"' | '$' | '`' | '\\' | '\'' | '\t' | '\n') || character.is_control()
    }) {
        return Err(DeployError(format!(
            "{label} may not contain {bad:?}: {prefix}"
        )));
    }
    Ok(())
}

/// One relocation pass, as the operator named it.
///
/// A struct and not six arguments because the CLI hands the same six words
/// through, and a `bool` pair plus three prefixes in positional form is how a
/// destination and a source end up swapped by a caller reading the wrong
/// line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelocatePlan {
    /// Store namespace holding both addresses.
    pub namespace: String,
    /// The mis-addressed key prefix.
    pub from: String,
    /// The key prefix the objects belong under; empty is the namespace root.
    pub to: String,
    /// Store root on the host, or [`DEFAULT_STORE_ROOT`](super::DEFAULT_STORE_ROOT)
    /// under its `$HOME`.
    pub store_root: Option<String>,
    /// Change bytes. False previews.
    pub apply: bool,
    /// Decide at most this many objects; 0 is all of them.
    pub limit: usize,
}
