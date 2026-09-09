//! Whether a candidate mount is spinning rust: the partition-name regex, the
//! /proc/mounts device lookup, the /sys/block base-name resolution, the
//! `queue/rotational` read, and the injectable and live forms of the answer
//! that ranks an SSD above a larger HDD.

use super::*;

static NVME_PART_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^(nvme\d+n\d+)p\d+$").expect("static regex compiles"));

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
