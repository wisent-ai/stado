//! Read a candidate's medium from the kernel's block-device hierarchy.
//! A partition uses its actual parent device, not a guessed name obtained
//! by stripping digits. Every failed lookup retains the operation and cause.

use super::*;

use std::os::unix::fs::FileTypeExt;

/// Pure parser: backing device for a mount point (last match in
/// /proc/mounts wins). Python `_mount_device`.
pub fn parse_mount_device(text: &str, mnt: &str) -> Result<Option<String>, String> {
    let mut selected = None;
    for line in text.lines() {
        let mut fields = line.split([' ', '\t']).filter(|field| !field.is_empty());
        let (Some(device), Some(mount_point)) = (fields.next(), fields.next()) else {
            return Err(format!("/proc/mounts row {line:?} has no source and mount point"));
        };
        if super::mounts::decode_mount_field(mount_point)? == mnt {
            selected = Some(device);
        }
    }
    selected.map(super::mounts::decode_mount_field)
        .transpose()
        .map(|device| device.map(std::borrow::Cow::into_owned))
}

/// Resolve the device the kernel says owns this queue. A partition's
/// `partition` attribute identifies it; its sysfs parent is the disk.
fn queue_device(sys_root: &Path, name: &std::ffi::OsStr) -> Result<PathBuf, String> {
    let declared = sys_root.join("class").join("block").join(name);
    let device = std::fs::canonicalize(&declared)
        .map_err(|error| format!("cannot resolve {}: {error}", declared.display()))?;
    let partition = device.join("partition");
    match std::fs::symlink_metadata(&partition) {
        Ok(_) => device.parent().map(Path::to_path_buf).ok_or_else(|| {
            format!("partition {} has no parent block device", device.display())
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(device),
        Err(error) => Err(format!("cannot inspect {}: {error}", partition.display())),
    }
}

/// The kernel's queue attribute is 1 for rotational, 0 for non-rotational.
/// https://www.kernel.org/doc/html/latest/block/queue-sysfs.html
fn rotational_file_value(device: &Path) -> Result<bool, String> {
    let path = device.join("queue").join("rotational");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    match text.trim() {
        "1" => Ok(true),
        "0" => Ok(false),
        other => Err(format!("{} holds {other:?}, not 0 or 1", path.display())),
    }
}

/// Whether the mount's backing block device is rotational (HDD), or why
/// that cannot be read. Python `_is_rotational` with injectable mounts text
/// + /sys root.
pub fn is_rotational_at(mounts_text: &str, sys_root: &Path, mnt: &str) -> Result<bool, String> {
    let dev = parse_mount_device(mounts_text, mnt)?
        .ok_or_else(|| format!("/proc/mounts has no entry for {mnt}"))?;
    let metadata = std::fs::metadata(&dev)
        .map_err(|error| format!("cannot inspect mount source {dev}: {error}"))?;
    if !metadata.file_type().is_block_device() {
        return Err(format!("{mnt} is backed by {dev}, not a block device"));
    }
    let resolved = std::fs::canonicalize(&dev)
        .map_err(|error| format!("cannot resolve {dev}: {error}"))?;
    let name = resolved.file_name()
        .ok_or_else(|| format!("{dev} resolves to {}, which names no device", resolved.display()))?;
    rotational_file_value(&queue_device(sys_root, name)?)
}

/// Python `_is_rotational` against the live /proc/mounts + /sys.
pub fn is_rotational(mnt: &str) -> Result<bool, String> {
    let mounts = std::fs::read_to_string("/proc/mounts")
        .map_err(|error| format!("cannot read /proc/mounts: {error}"))?;
    is_rotational_at(&mounts, Path::new("/sys"), mnt)
}
