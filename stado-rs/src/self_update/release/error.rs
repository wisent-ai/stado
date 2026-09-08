//! The failure and outcome vocabulary every self-update stage reports in.

use std::path::PathBuf;

/// Self-update failure. Every variant aborts before any binary is
/// replaced; the caller logs and keeps running the old binary.
#[derive(Debug, thiserror::Error)]
pub enum SelfUpdateError {
    /// This OS/arch has no published release triple.
    #[error("no release triple for platform {os}-{arch} (supported: linux-amd64, darwin-arm64)")]
    UnsupportedPlatform {
        os: &'static str,
        arch: &'static str,
    },
    /// The release channel/object could not be read.
    #[error("release fetch failed: {0}")]
    Fetch(String),
    /// SHA256SUMS is not parseable coreutils format.
    #[error("malformed SHA256SUMS: {0}")]
    MalformedSums(String),
    /// SHA256SUMS carries no entry for a binary we need to replace.
    #[error("SHA256SUMS has no entry for {0}")]
    MissingSum(String),
    /// A downloaded binary does not match its published checksum.
    #[error("sha256 mismatch for {name}: expected {expected}, got {actual}")]
    HashMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    /// `current_exe` has no usable parent directory.
    #[error("cannot determine install dir of {path}", path = .0.display())]
    NoInstallDir(PathBuf),
    /// The running binary's file name is not a published release binary,
    /// so there is nothing safe to download over it.
    #[error("running binary {0:?} is not one of the published release binaries")]
    UnknownBinary(String),
    /// Probed by creating the staging temp dir inside the install dir
    /// before any download starts.
    #[error("install dir {path} is not writable: {1}", path = .0.display())]
    InstallDirNotWritable(PathBuf, String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What [`self_update`] did.
///
/// [`self_update`]: crate::self_update::self_update
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// The release was downloaded, verified and installed; the caller
    /// should [`reexec`].
    ///
    /// [`reexec`]: crate::self_update::reexec
    Updated { from: String, to: String },
    /// The configured exact version is not strictly newer than the installed
    /// binary.
    UpToDate { installed: String, latest: String },
}
