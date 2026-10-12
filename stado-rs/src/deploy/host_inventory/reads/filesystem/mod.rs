//! What the inventory reads off the disk: the Skarbiec vault files as
//! metadata only, and the lstat metadata every fixed path and Cargo entry is
//! reported through.

use serde::{Deserialize, Serialize};

/// One `$HOME/.stado/*.vault*.json` file, as METADATA ONLY.
///
/// This is the whole shape of the vault answer, and it is deliberately
/// small. A Skarbiec vault is a file of secrets; "which vaults are on this
/// host" is answerable from `stat(2)`, so it is answered from `stat(2)`.
/// There is no field here that could carry a byte of ciphertext, an item
/// id, a consumer name or a token, because the script never opens the file
/// — not to count, not to validate, not to peek.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaultFile {
    /// The basename, sanitized by the same shell `sanitize` as every other
    /// value the script reports.
    pub name: String,
    /// [`VAULT_REGULAR`], [`VAULT_REFUSED_SYMLINK`] or
    /// [`VAULT_REFUSED_NOT_REGULAR`].
    pub state: String,
    /// Size in bytes. Typed as an integer, so a `bytes` the host did not
    /// state as a number fails the whole parse instead of arriving as a
    /// quiet zero.
    pub bytes: u64,
    /// Permission bits in octal, e.g. `600`. `unknown` only when the file
    /// disappeared between the glob and the stat.
    pub mode: String,
    /// No group bits and no other bits. A vault the group can read is an
    /// incident, not a cosmetic detail, which is why this is a field of its
    /// own rather than something the operator derives from `mode`.
    pub owner_only: bool,
}

/// Metadata for one fixed filesystem entry, collected without following it.
///
/// Numeric fields are absent when lstat could not read all of them. `kind`
/// still distinguishes an absent path from one whose metadata read failed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesystemMetadata {
    pub name: String,
    /// `read` or `sanitized`. The latter makes Cargo inventory incomplete:
    /// its bounded display name is not exact membership.
    pub name_state: String,
    /// `missing`, `symlink`, `directory`, `regular`, `other`, or
    /// `uninspected` when a parent was refused.
    pub kind: String,
    /// `read`, `missing`, `unavailable`, `malformed`, or a
    /// `refused_parent_*` state.
    pub metadata_state: String,
    pub bytes: Option<u64>,
    /// Permission bits in octal, or `unknown`.
    pub mode: String,
    pub uid: Option<u64>,
    pub gid: Option<u64>,
    pub modified_epoch: Option<i64>,
    /// The link text for a symlink. Empty for every other kind and when
    /// readlink itself could not answer.
    pub symlink_target: String,
    /// `read`, `not_symlink`, `unavailable`, or `sanitized`.
    pub symlink_target_state: String,
}

/// The states of the inventory script this module judges. A state word is
/// parsed into this type; a word it does not name (`unavailable`,
/// `malformed`, a `refused_parent_*` state) parses to `None`, a read that did
/// not finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::deploy::host_inventory) enum ObservedState {
    /// Read as the host has it.
    Read,
    /// Not there.
    Missing,
    /// No link text, because the entry is not a symbolic link.
    NotSymlink,
    /// Changed by the script's sanitizer, so not exact.
    Sanitized,
}

impl ObservedState {
    pub(in crate::deploy::host_inventory) fn parse(word: &str) -> Option<Self> {
        Self::deserialize(serde_json::Value::String(word.to_string())).ok()
    }
}

/// The read answered: the entry was read, or it is not there.
pub(in crate::deploy::host_inventory) fn answered(state: &str) -> bool {
    matches!(
        ObservedState::parse(state),
        Some(ObservedState::Read | ObservedState::Missing)
    )
}

/// Whether a name and link text arrived exactly as the host has them: the
/// script's sanitizer marks any value it had to change as `sanitized`.
pub(in crate::deploy::host_inventory) fn metadata_exact(metadata: &FilesystemMetadata) -> bool {
    ObservedState::parse(&metadata.name_state) != Some(ObservedState::Sanitized)
        && ObservedState::parse(&metadata.symlink_target_state) != Some(ObservedState::Sanitized)
}

/// Name, metadata and link text all arrived as the host has them.
pub(in crate::deploy::host_inventory) fn read_exactly(metadata: &FilesystemMetadata) -> bool {
    let read = |state: &str| ObservedState::parse(state) == Some(ObservedState::Read);
    read(&metadata.name_state)
        && read(&metadata.metadata_state)
        && matches!(
            ObservedState::parse(&metadata.symlink_target_state),
            Some(ObservedState::Read | ObservedState::NotSymlink)
        )
}
