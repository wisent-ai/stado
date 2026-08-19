//! Auto-redirect agent staging away from a RAM-backed /tmp.
//!
//! Port of `stado/providers/local/disk/staging.py`.
//!
//! When `/tmp` is a tmpfs (every byte staged there counts as RAM), the
//! agent's multi-GB raw-activation jobs accumulate in RAM, drive the
//! process to OOM territory, and exit `status=1` — losing every in-flight
//! job to requeue. This module runs at agent startup, detects that
//! condition, and points TMPDIR at the largest disk-backed mount the agent
//! user can actually traverse and write, creating a `wisent-staging`
//! subdir there. Children inherit the env, so every job stages on disk for
//! free. (Python also assigns `tempfile.tempdir`; Rust has no analog — the
//! env var is what children inherit.)
//!
//! Fully automatic, resource-linked. No hardcoded paths, no concurrency
//! cap. No-op when /tmp is already disk-backed or TMPDIR is already set to
//! a non-/tmp path. When running as root the agent can also chmod o+x
//! parent dirs to recover an otherwise-skipped large mount (e.g. a Vast
//! host where /var/lib/docker is 0710).

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Python `_BAD_FS`: pseudo/RAM filesystems that must never host staging.
const BAD_FS: &[&str] = &[
    "tmpfs",
    "devtmpfs",
    "proc",
    "sysfs",
    "cgroup",
    "cgroup2",
    "fusectl",
    "configfs",
    "debugfs",
    "pstore",
    "bpf",
    "ramfs",
    "mqueue",
    "tracefs",
    "securityfs",
    "autofs",
    "nsfs",
    "binfmt_misc",
    "hugetlbfs",
    "rpc_pipefs",
    "fuse.gvfsd-fuse",
    "squashfs",
    "iso9660",
];

static NVME_PART_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(nvme\d+n\d+)p\d+$").expect("static regex compiles"));

/// Python `_tmp_is_tmpfs` (`stat -f -c %T /tmp`).
pub async fn tmp_is_tmpfs() -> bool {
    match tokio::process::Command::new("stat")
        .args(["-f", "-c", "%T", "/tmp"])
        .output()
        .await
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim() == "tmpfs",
        Err(_) => false,
    }
}

/// Pure parser for /proc/mounts: disk-backed, read-write mount points.
/// Python the filtering loop of `_candidate_mounts`.
pub fn parse_mounts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let (mnt, fstype, opts) = (parts[1], parts[2], parts[3]);
        if BAD_FS.contains(&fstype) {
            continue;
        }
        if opts.split(',').any(|opt| opt == "ro") {
            continue;
        }
        if mnt.starts_with("/boot") {
            continue;
        }
        out.push(mnt.to_string());
    }
    out
}

/// Disk-backed, read-write mount points (from /proc/mounts).
/// Python `_candidate_mounts`.
pub fn candidate_mounts() -> Vec<String> {
    std::fs::read_to_string("/proc/mounts")
        .map(|text| parse_mounts(&text))
        .unwrap_or_default()
}

/// Python `_free_gb` (`shutil.disk_usage(path).free`). -1.0 on error.
pub fn free_gb(path: &Path) -> f64 {
    match nix::sys::statvfs::statvfs(path) {
        Ok(stat) => stat.blocks_available() as f64 * stat.fragment_size() as f64 / 1024f64.powi(3),
        Err(_) => -1.0,
    }
}

/// Python `_writable_for_self`: create+remove a probe file in `path`.
pub fn writable_for_self(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let probe = path.join(format!(".wc_staging_probe_{}", std::process::id()));
    match std::fs::write(&probe, "x") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Python `_add_other_exec`: chmod o+x, then verify the bit stuck.
fn add_other_exec(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(md) = std::fs::metadata(path) else {
        return false;
    };
    let mode = md.permissions().mode();
    let new_mode = mode | 0o001;
    if new_mode != mode
        && std::fs::set_permissions(path, std::fs::Permissions::from_mode(new_mode)).is_err()
    {
        return false;
    }
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o001 != 0)
        .unwrap_or(false)
}

/// Python `_try_repair_traversal`: o+x every parent of `target` (root only).
fn try_repair_traversal(target: &Path, log_fn: &mut dyn FnMut(&str)) {
    let mut dir = target.parent().map(Path::to_path_buf);
    while let Some(p) = dir {
        if p.as_os_str().is_empty() || p == Path::new("/") {
            break;
        }
        if !add_other_exec(&p) {
            log_fn(&format!(
                "staging: cannot chmod o+x {} (need root); admin should add o+x",
                p.display()
            ));
        }
        dir = p.parent().map(Path::to_path_buf);
    }
}

/// Effective uid via libc (nix's typed wrappers need features outside the
/// port's allowed set).
fn euid() -> u32 {
    // SAFETY: geteuid is always successful and async-signal-safe.
    unsafe { nix::libc::geteuid() }
}

/// One /etc/passwd row (only the fields the staging repair needs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswdEntry {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
}

/// Pure /etc/passwd parser, standing in for Python's `pwd` module (nix's
/// `user` feature is not in the port's dependency set).
pub fn parse_passwd(text: &str) -> Vec<PasswdEntry> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            fields.next()?; // password placeholder
            let uid = fields.next()?.parse().ok()?;
            let gid = fields.next()?.parse().ok()?;
            Some(PasswdEntry {
                name: name.to_string(),
                uid,
                gid,
            })
        })
        .collect()
}

fn read_passwd() -> Vec<PasswdEntry> {
    std::fs::read_to_string("/etc/passwd")
        .map(|text| parse_passwd(&text))
        .unwrap_or_default()
}

/// Python `_agent_user`: WISENT_STAGING_USER override, else the passwd
/// name for the euid, else the numeric euid (Python's KeyError fallback).
pub fn agent_user() -> String {
    if let Ok(explicit) = std::env::var("WISENT_STAGING_USER") {
        let explicit = explicit.trim();
        if !explicit.is_empty() {
            return explicit.to_string();
        }
    }
    let uid = euid();
    read_passwd()
        .into_iter()
        .find(|entry| entry.uid == uid)
        .map(|entry| entry.name)
        .unwrap_or_else(|| uid.to_string())
}

/// Python `_chown_if_root`: hand the staging dir to the agent user.
fn chown_if_root(path: &Path, user: &str) {
    if euid() != 0 {
        return;
    }
    let Some(entry) = read_passwd().into_iter().find(|entry| entry.name == user) else {
        return;
    };
    use std::os::unix::fs::MetadataExt;
    let Ok(md) = std::fs::metadata(path) else {
        return;
    };
    if md.uid() != entry.uid {
        if let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) {
            // SAFETY: c_path is a valid NUL-terminated path; errors are
            // deliberately ignored (Python's bare `except OSError: pass`).
            unsafe { nix::libc::chown(c_path.as_ptr(), entry.uid, entry.gid) };
        }
    }
}

/// Pure parser: backing device for a mount point (last match in
/// /proc/mounts wins). Python `_mount_device`.
pub fn parse_mount_device(text: &str, mnt: &str) -> Option<String> {
    let mut dev = None;
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[1] == mnt {
            dev = Some(parts[0].to_string());
        }
    }
    dev
}

/// Pure: block-device base name for a partition name
/// (Python the `nvme...pN` regex + trailing-digit strip of `_is_rotational`).
pub fn block_base_name(name: &str) -> String {
    if let Some(caps) = NVME_PART_RE.captures(name) {
        return caps[1].to_string();
    }
    name.trim_end_matches(|c: char| c.is_ascii_digit())
        .to_string()
}

/// Pure: resolve the /sys/block base dir for a device name — the name
/// itself when it exists under sys_root, else the partition-stripped base.
pub fn resolved_block_base(sys_root: &Path, name: &str) -> String {
    if sys_root.join("block").join(name).exists() {
        name.to_string()
    } else {
        block_base_name(name)
    }
}

/// Pure: read /sys/block/<base>/queue/rotational; unreadable -> true (HDD).
pub fn rotational_file_value(sys_root: &Path, base: &str) -> bool {
    match std::fs::read_to_string(
        sys_root
            .join("block")
            .join(base)
            .join("queue")
            .join("rotational"),
    ) {
        Ok(text) => text.trim() == "1",
        Err(_) => true,
    }
}

/// True if the mount's backing block device is rotational (HDD).
/// Python `_is_rotational` with injectable mounts text + /sys root.
pub fn is_rotational_at(mounts_text: &str, sys_root: &Path, mnt: &str) -> bool {
    // Unknown resolves to True so an unidentifiable device is treated as an
    // HDD and never wins over a confirmed SSD. Multi-GB shard staging is
    // write-throughput bound, so an SSD is strongly preferable to a larger
    // HDD.
    let Some(dev) = parse_mount_device(mounts_text, mnt) else {
        return true;
    };
    if !dev.starts_with("/dev/") {
        return true;
    }
    // os.path.basename(os.path.realpath(dev)).
    let Some(name) = std::fs::canonicalize(&dev)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
    else {
        return true;
    };
    let base = resolved_block_base(sys_root, &name);
    rotational_file_value(sys_root, &base)
}

/// Python `_is_rotational` against the live /proc/mounts + /sys.
pub fn is_rotational(mnt: &str) -> bool {
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    is_rotational_at(&mounts, Path::new("/sys"), mnt)
}

/// Detect a RAM-backed /tmp and redirect TMPDIR to disk. Returns the
/// chosen path, or None when nothing was changed.
/// Python `setup_agent_staging`.
pub async fn setup_agent_staging(log_fn: &mut dyn FnMut(&str)) -> Option<String> {
    let explicit = std::env::var("TMPDIR")
        .unwrap_or_default()
        .trim()
        .to_string();
    if !explicit.is_empty() && !explicit.starts_with("/tmp") {
        log_fn(&format!(
            "staging: TMPDIR already set to {explicit}; keeping it"
        ));
        return Some(explicit);
    }
    if !tmp_is_tmpfs().await {
        return None;
    }
    let tmp_free = free_gb(Path::new("/tmp"));
    let user = agent_user();
    let mut best: Option<(String, f64, bool)> = None;
    for mnt in candidate_mounts() {
        let target: PathBuf = Path::new(&mnt).join("wisent-staging");
        if std::fs::create_dir_all(&target).is_err() {
            continue;
        }
        chown_if_root(&target, &user);
        if !writable_for_self(&target) {
            if euid() == 0 {
                try_repair_traversal(&target, log_fn);
                if !writable_for_self(&target) {
                    continue;
                }
            } else {
                log_fn(&format!(
                    "staging: candidate {} not writable by current user (parent perms?); skipping",
                    target.display()
                ));
                continue;
            }
        }
        let free = free_gb(&target);
        if free <= tmp_free {
            continue;
        }
        let rotational = is_rotational(&mnt);
        // Rank: prefer non-rotational (SSD/NVMe) over rotational (HDD);
        // break ties by free space. Shard staging is write-throughput
        // bound, so a smaller SSD beats a larger HDD.
        let better = match &best {
            None => true,
            Some((_, best_free, best_rotational)) => {
                let (rank, best_rank) = (!rotational, !*best_rotational);
                (rank && !best_rank) || (rank == best_rank && free > *best_free)
            }
        };
        if better {
            best = Some((target.to_string_lossy().into_owned(), free, rotational));
        }
    }
    let Some((target, free, rotational)) = best else {
        log_fn(
            "staging: /tmp is tmpfs but no larger writable disk-backed mount found; \
             staging stays on /tmp (RAM). Crashes possible.",
        );
        return None;
    };
    // Children inherit the env, so every job stages on disk for free.
    std::env::set_var("TMPDIR", &target);
    log_fn(&format!(
        "staging: redirected TMPDIR /tmp(tmpfs,{tmp_free:.0}G) -> {target} \
         ({}-backed, {free:.0}G free)",
        if rotational { "HDD" } else { "SSD" }
    ));
    Some(target)
}

