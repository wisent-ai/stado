//! The fixed remote program, assembled from its two shell fragments.
//!
//! The prologue checks both roots, records free space and runs the pass with
//! the host's own Stado (`stado host backup-audit-local`, see
//! [`super::local`]); the epilogue prunes emptied directories and records free
//! space again. One Stado process walks the replica rather than shell with
//! per-file `stat`: the replica holds tens of thousands of objects, and a fork
//! or three for each of them is the slow part. The roots arrive as arguments
//! the host's shell expands, so nothing operator-supplied is spliced into a
//! program text beyond the quoted markers [`super::remote_script`] fills.

/// The fixed remote program. It walks the replica once, compares sizes,
/// hashes every same-size pair, and — only under `@RECLAIM@` with `@APPLY@` —
/// unlinks the ones it has just proven identical.
pub(super) const REMOTE_SCRIPT_TEMPLATE: &str = concat!(
    include_str!("program_01_shell_prologue.sh"),
    include_str!("program_02_shell_epilogue.sh"),
);
