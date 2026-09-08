//! The fixed remote program, assembled from its three fragments.
//!
//! The program is one shell prologue that exports the roots, the `python3`
//! pass that reads both stores, and one shell epilogue that prunes emptied
//! directories and reports free space. Each fragment is a file rather than a
//! Rust literal because the program is longer than a source file may be, and
//! `concat!` of `include_str!` reassembles it at compile time with the bytes
//! unchanged: the marker substitution in
//! [`remote_script`](super::remote_script) still sees one program text.

/// The fixed remote program. It walks the replica once, compares sizes, hashes
/// same-size pairs until its deadline, and — only under `@RECLAIM@` with
/// `@APPLY@` — unlinks the ones it has just proven identical.
///
/// One `python3` process rather than shell with per-file `stat`: the replica
/// holds tens of thousands of objects, and a fork or three for each of them
/// does not finish inside the fleet channel's 120-second budget. The first
/// version of this script did exactly that and timed out twice before reaching
/// a single hash. The roots arrive through the environment so nothing operator-
/// supplied is ever spliced into a program text.
pub(super) const REMOTE_SCRIPT_TEMPLATE: &str = concat!(
    include_str!("program_01_shell_prologue.sh"),
    include_str!("program_02_python_pass.py"),
    include_str!("program_03_shell_epilogue.sh"),
);
