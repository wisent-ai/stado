//! What this machine itself says, read with the operating system's own tools.
//!
//! Every byte figure the product reports about a cleanup scope is compared
//! against one of these rather than against a second copy of the product's
//! own arithmetic: `stat`'s allocated blocks for the bytes a pass charges,
//! `du -sk` for what a tree occupies, `df -Pk /` for the free space the
//! watermarks are measured against, and `tmutil` for the local snapshots both
//! reads report. A zeroed, copied or invented number therefore fails here.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

/// The fixed PATH these readers and the binary under test are given.
pub const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// A `du -sk` block is 1024 bytes; `stat` reports 512-byte blocks. Both are
/// the units the tools themselves print and neither is tunable.
pub const KIB: i64 = 1024;
const STAT_BLOCK: i64 = 512;

/// The six columns of one POSIX `df` row: filesystem, 1024-blocks, used,
/// available, capacity, mount point.
const DF_COLUMNS: usize = 6;

/// One `df -Pk /` row, as the operating system reported it to the test.
pub struct DiskFacts {
    pub blocks_kb: i64,
    pub available_kb: i64,
}

/// `df -Pk /` — the same tool, options and filesystem the product's own disk
/// section reads.
pub fn df_root() -> DiskFacts {
    let output = tool("/bin/df", &["-Pk", "/"]);
    let row = output.lines().nth(1).expect("df printed a row for /");
    let fields: Vec<&str> = row.split_whitespace().collect();
    assert_eq!(
        fields.len(),
        DF_COLUMNS,
        "df -Pk printed an unexpected row: {row}"
    );
    DiskFacts {
        blocks_kb: fields[1].parse().expect("df printed 1024-blocks"),
        available_kb: fields[3].parse().expect("df printed available blocks"),
    }
}

/// `du -sk PATH`, in bytes.
pub fn du_bytes(path: &Path) -> i64 {
    let output = tool("/usr/bin/du", &["-sk", &path.to_string_lossy()]);
    let kib: i64 = output
        .split_whitespace()
        .next()
        .expect("du printed a size")
        .parse()
        .expect("du printed a KiB count");
    kib * KIB
}

/// The bytes the operating system says a tree occupies, counted the way the
/// janitor's own `tree_bytes` counts them: allocated blocks of every
/// non-directory entry below the tree.
pub fn allocated_bytes(tree: &Path) -> i64 {
    let mut total = 0;
    for entry in fs::read_dir(tree).expect("read the scope this case created") {
        let entry = entry.expect("read the scope's entry");
        let info = entry.metadata().expect("stat the scope's entry");
        if info.is_dir() {
            total += allocated_bytes(&entry.path());
            continue;
        }
        total += info.blocks() as i64 * STAT_BLOCK;
    }
    total
}

/// How many local APFS snapshots this machine holds, as `tmutil` names them.
pub fn local_snapshot_count() -> i64 {
    tool("/usr/bin/tmutil", &["listlocalsnapshots", "/"])
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("com.apple."))
        .count() as i64
}

/// This machine's kernel hostname, which is what the fixture registry
/// declares, so the product resolves its one target to the current host and
/// really runs the local path.
pub fn hostname() -> String {
    let hostname = tool("/bin/hostname", &[]).trim().to_lowercase();
    assert!(!hostname.is_empty(), "this host has no hostname");
    hostname
}

/// Run one of the operating system's own read-only tools, or fail the case. A
/// missing or failing native reader is never skipped: without it there is no
/// answer to compare the product against.
fn tool(program: &str, args: &[&str]) -> String {
    let output = Command::new(program)
        .args(args)
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .output()
        .unwrap_or_else(|error| panic!("blocked: {program} could not start: {error}"));
    assert!(
        output.status.success(),
        "blocked: {program} {args:?} failed: {}",
        said(&output.stderr),
    );
    said(&output.stdout)
}

pub fn said(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
