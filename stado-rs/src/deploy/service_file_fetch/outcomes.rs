//! The vocabulary of a fetch: the `status` word, the per-file states, the
//! integrity verdicts and the size ceiling.

/// `status` for a report that came back whole.
pub const OK_STATUS: &str = "file_fetch";

/// The file was a regular file the login user could read, and its bytes came
/// back.
pub const FILE_READ: &str = "read";
/// The path resolved outside the target's home and was never opened.
pub const FILE_REFUSED_OUTSIDE_HOME: &str = "refused_outside_home";
/// The path is a symlink. Never followed, for
/// [`super::service_env_file`](crate::deploy::service_env_file)'s
/// reason: a symlink under a home directory is how a read of `~/.config/x.env`
/// becomes a read of `~/.ssh/id_ed25519`.
pub const FILE_REFUSED_SYMLINK: &str = "refused_symlink";
/// There is no regular file at the path.
pub const FILE_MISSING: &str = "missing";
/// The file exists and the login user cannot read it.
pub const FILE_UNREADABLE: &str = "unreadable";
/// The file is larger than [`MAX_FETCH_BYTES`] and was never read. A fetch
/// that returned a prefix without saying so would be the worst possible answer
/// here: the digest would match the prefix and the caller would commit a
/// truncated program.
pub const FILE_REFUSED_TOO_LARGE: &str = "refused_too_large";
/// The host has neither SHA-256 tool, so no digest could be computed and
/// nothing was transferred. A fetch with no digest is not a fetch this command
/// performs.
pub const FILE_NO_HASHER: &str = "no_hasher";
/// The file was readable and its bytes could not be encoded for transport.
pub const FILE_ENCODE_FAILED: &str = "encode_failed";

/// Host digest and local digest agree over the decoded bytes.
pub const INTEGRITY_VERIFIED: &str = "verified";
/// The two digests disagree, or the payload did not decode. Nothing is
/// written.
pub const INTEGRITY_MISMATCH: &str = "mismatch";
/// No bytes were transferred, so there was nothing to verify.
pub const INTEGRITY_UNVERIFIED: &str = "unverified";

/// The largest file this command will move.
///
/// Sized for what it is for: operator scripts, unit files, launch wrappers and
/// configuration — the unversioned text a repository should have been holding
/// all along. A release artifact belongs in the object store, travels with a
/// published digest, and has `stado storage` and `service update` to move it;
/// routing one through a control-plane process's `stdout` would be a second,
/// worse delivery path for bytes that already have one.
pub const MAX_FETCH_BYTES: u64 = 1_048_576;
