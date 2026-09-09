//! Which mounts may host staging: the pseudo/RAM filesystem denylist, the
//! tmpfs test on `/tmp` that arms the redirect, the /proc/mounts filter and
//! its live reader, the free space of a candidate, the write probe the agent
//! user has to pass, and the root-only traversal repair that recovers a
//! candidate whose parents are not world-traversable.

use super::*;

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
    "overlay",
];

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
pub(super) fn try_repair_traversal(target: &Path, log_fn: &mut dyn FnMut(&str)) {
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
