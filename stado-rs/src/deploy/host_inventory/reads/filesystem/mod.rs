//! What the inventory reads off the disk: the Skarbiec vault files as
//! metadata only, the lstat metadata every fixed path and Cargo entry is
//! reported through, and the cap every reported string passes.

use serde::{Deserialize, Serialize};

use super::super::*;

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

/// Cap one reported string at [`MAX_FIELD_CHARS`] characters, marking the cut.
pub(in crate::deploy::host_inventory) fn clamp(value: &mut String) {
    if value.chars().count() <= MAX_FIELD_CHARS {
        return;
    }
    let keep = MAX_FIELD_CHARS - ELLIPSIS.chars().count();
    let end = value
        .char_indices()
        .nth(keep)
        .map_or(value.len(), |(index, _)| index);
    value.truncate(end);
    value.push_str(ELLIPSIS);
}

/// Cap one vault section: the file count first, then every string in it.
///
/// `seen` is raised to the number of entries that actually arrived before
/// the cut, so an over-long list from a misbehaving host is reported as
/// truncated rather than as a section that grew past its own cap.
pub(in crate::deploy::host_inventory) fn clamp_vault_section(
    files: &mut Vec<VaultFile>,
    seen: &mut u64,
) {
    *seen = (*seen).max(files.len() as u64);
    files.truncate(MAX_VAULT_FILES);
    for file in files {
        clamp(&mut file.name);
        clamp(&mut file.state);
        clamp(&mut file.mode);
    }
}

pub(in crate::deploy::host_inventory) fn clamp_filesystem_metadata(
    metadata: &mut FilesystemMetadata,
) -> bool {
    let exact = metadata.name.chars().count() <= MAX_FIELD_CHARS
        && metadata.symlink_target.chars().count() <= MAX_FIELD_CHARS;
    clamp(&mut metadata.name);
    clamp(&mut metadata.name_state);
    clamp(&mut metadata.kind);
    clamp(&mut metadata.metadata_state);
    clamp(&mut metadata.mode);
    clamp(&mut metadata.symlink_target);
    clamp(&mut metadata.symlink_target_state);
    exact
}
