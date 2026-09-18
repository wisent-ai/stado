//! `stado space volume mount TARGET --device NAME --mount-point PATH`: give a
//! disk the host already has a place in its filesystem tree, durably.
//!
//! On 2026-09-18 ubuntu-server-rtx-pro-6000 carried `/dev/sdb1`, 16.4 TiB of
//! xfs, attached and mounted nowhere, while the fleet refused a 22 GiB build
//! for want of room on the 98 GiB root volume. The disk had been mounted at
//! `/mnt/wd16tb` two weeks earlier and had come back after a reattach with
//! no fstab line to bring it up. `stado space report` and `stado host gates`
//! now name such a disk ([`crate::deploy::host_gates::DISK_ATTACHED_UNMOUNTED`]);
//! this module is the command that mounts it.
//!
//! The program mounts, it never formats: a device with no filesystem is
//! refused by name, because the one command that can write a filesystem
//! over whatever bytes a disk holds does not belong beside the one that
//! reads them. The fstab line is by UUID, `nofail`, and marked
//! `# stado-volume`, so the host boots without the disk and finds it again
//! with it. The existing fstab is copied beside itself before the first
//! write, the same way the stream library bind does.
//!
//! Like [`crate::deploy::host_recovery`]'s script, the remote program is a
//! fixed text with the two operator values spliced in shell-quoted.

use serde_json::{json, Map, Value};

use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Substitution points in [`PROGRAM`]; both are shell-quoted before splicing.
const DEVICE_MARK: &str = "@DEVICE@";
const MOUNT_POINT_MARK: &str = "@MOUNT_POINT@";

/// The fstab comment that marks a line this command wrote.
pub const FSTAB_TAG: &str = "# stado-volume";

/// The fstab dump and fsck-order fields of the line this command writes:
/// no dump, checked after the root filesystem, as every data volume's
/// fstab line reads.
const FSTAB_DUMP_AND_PASS: &str = "0 2";

/// The fixed program. Every finding is a tab-separated marker line, in the
/// protocol [`host_channel::marker_fields`] reads; `ERROR` lines end the run.
const PROGRAM: &str = r#"set -u
device=@DEVICE@
mount_point=@MOUNT_POINT@
node="/dev/$device"
if [ ! -x /usr/bin/lsblk ]; then
  printf 'ERROR\t%s\n' 'this host has no lsblk; only a Linux host can be asked to mount a block device'
  exit 1
fi
if [ ! -b "$node" ]; then
  printf 'ERROR\t%s is not a block device on this host\n' "$node"
  exit 1
fi
fstype=$(/usr/bin/lsblk -no FSTYPE "$node" 2>/dev/null | sed -n 1p)
uuid=$(/usr/bin/lsblk -no UUID "$node" 2>/dev/null | sed -n 1p)
current=$(/usr/bin/lsblk -no MOUNTPOINT "$node" 2>/dev/null | sed -n 1p)
size=$(/usr/bin/lsblk -bno SIZE "$node" 2>/dev/null | sed -n 1p)
printf 'STADO_VOLUME_DEVICE\t%s\t%s\t%s\t%s\n' "$node" "${fstype:-}" "${uuid:-}" "${size:-}"
if [ -z "${fstype:-}" ]; then
  printf 'ERROR\t%s holds no filesystem; this command mounts and never formats\n' "$node"
  exit 1
fi
case "$fstype" in
  LVM2_member|crypto_LUKS|swap)
    printf 'ERROR\t%s is a %s, not a mountable filesystem\n' "$node" "$fstype"
    exit 1 ;;
esac
if [ -z "${uuid:-}" ]; then
  printf 'ERROR\t%s reports no filesystem UUID, so no durable fstab line can name it\n' "$node"
  exit 1
fi
if [ -n "${current:-}" ] && [ "$current" != "$mount_point" ]; then
  printf 'ERROR\t%s is already mounted at %s; unmount it first or mount it there\n' "$node" "$current"
  exit 1
fi
if [ -e "$mount_point" ] && [ ! -d "$mount_point" ]; then
  printf 'ERROR\t%s exists and is not a directory\n' "$mount_point"
  exit 1
fi
mkdir -p "$mount_point" || { printf 'ERROR\tcannot create %s\n' "$mount_point"; exit 1; }
line="UUID=$uuid $mount_point $fstype defaults,nofail @FSTAB_DUMP_AND_PASS@ @FSTAB_TAG@"
fstab_written=0
if grep -Eq "^UUID=$uuid[[:space:]]" /etc/fstab 2>/dev/null; then
  existing=$(grep -E "^UUID=$uuid[[:space:]]" /etc/fstab | sed -n 1p)
  existing_point=$(printf '%s\n' "$existing" | awk '{ print $2 }')
  if [ "$existing_point" != "$mount_point" ]; then
    printf 'ERROR\t/etc/fstab already mounts UUID=%s at %s, not %s\n' "$uuid" "$existing_point" "$mount_point"
    exit 1
  fi
else
  cp -p /etc/fstab "/etc/fstab.before-stado-volume-$(date -u +%Y%m%d)" || { printf 'ERROR\tcannot back up /etc/fstab\n'; exit 1; }
  printf '%s\n' "$line" >>/etc/fstab || { printf 'ERROR\tcannot write /etc/fstab\n'; exit 1; }
  fstab_written=1
fi
printf 'STADO_VOLUME_FSTAB\t%s\t%s\n' "$fstab_written" "$line"
if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload >/dev/null 2>&1 || true
fi
mounted_now=0
if [ -z "${current:-}" ]; then
  mount "$mount_point" 2>&1 | sed 's/^/STADO_VOLUME_MOUNT_OUTPUT\t/'
  mounted_now=1
fi
if ! awk -v point="$mount_point" '$2 == point { found = 1 } END { exit !found }' /proc/self/mounts; then
  printf 'ERROR\t%s is not mounted after mount %s; read the mount output above\n' "$node" "$mount_point"
  exit 1
fi
printf 'STADO_VOLUME_MOUNTED\t%s\n' "$mounted_now"
/bin/df -Pk "$mount_point" 2>/dev/null | awk 'NR == 2 { printf "STADO_VOLUME_USAGE\t%s\t%s\t%s\t%s\t%s\t%s\n", $1, $2, $3, $4, $5, $6 }'
"#;

/// What the host said about the device and the mount.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VolumeMount {
    pub device: String,
    pub fstype: String,
    pub uuid: String,
    pub size_bytes: i64,
    /// The fstab line in force for this UUID, and whether this run wrote it.
    pub fstab_line: String,
    pub fstab_written: bool,
    /// Whether this run performed the mount, as opposed to finding it.
    pub mounted_now: bool,
    pub mount_output: Vec<String>,
    pub filesystem: String,
    pub blocks_kb: String,
    pub available_kb: String,
    pub mounted_on: String,
    pub error: Option<String>,
}

/// A device name the program will accept: one `/dev` leaf such as `sdb1` or
/// `nvme0n1p2`, never a path and never a word the shell could read.
pub fn validate_device(device: &str) -> Result<(), String> {
    let acceptable = !device.is_empty()
        && device
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
    if acceptable {
        Ok(())
    } else {
        Err(format!(
            "--device names one /dev leaf such as sdb1 or nvme0n1p2, not {device:?}"
        ))
    }
}

/// A mount point the program will accept: an absolute, normalised path of
/// plain components, and never `/`, `/boot` or a path inside the system
/// trees a mount would shadow.
pub fn validate_mount_point(path: &str) -> Result<(), String> {
    let Some(relative) = path.strip_prefix('/') else {
        return Err(format!(
            "--mount-point is an absolute directory path such as /mnt/wd16tb, not {path:?}"
        ));
    };
    if relative.is_empty() || relative.ends_with('/') {
        return Err(format!(
            "--mount-point is an absolute directory path such as /mnt/wd16tb, not {path:?}"
        ));
    }
    let components: Vec<&str> = relative.split('/').collect();
    if components.iter().any(|component| {
        component.is_empty()
            || *component == "."
            || *component == ".."
            || !component
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    }) {
        return Err(format!(
            "--mount-point is a plain absolute path of letters, digits, '-', '_' and '.', not {path:?}"
        ));
    }
    if matches!(
        components[0],
        "boot" | "dev" | "etc" | "proc" | "run" | "sys" | "usr" | "bin" | "sbin" | "lib" | "var"
    ) {
        return Err(format!(
            "--mount-point {path:?} is under a system tree; mount a data disk under /mnt, /srv, /data or /home"
        ));
    }
    Ok(())
}

/// Fold the program's marker lines into a reading.
pub fn parse_output(stdout: &str) -> VolumeMount {
    let mut mount = VolumeMount::default();
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_VOLUME_DEVICE", node, fstype, uuid, size] => {
                mount.device = (*node).to_string();
                mount.fstype = (*fstype).to_string();
                mount.uuid = (*uuid).to_string();
                mount.size_bytes = size.parse().unwrap_or_default();
            }
            ["STADO_VOLUME_FSTAB", written, line] => {
                mount.fstab_written = *written == "1";
                mount.fstab_line = (*line).to_string();
            }
            ["STADO_VOLUME_MOUNT_OUTPUT", text] => mount.mount_output.push((*text).to_string()),
            ["STADO_VOLUME_MOUNTED", now] => mount.mounted_now = *now == "1",
            ["STADO_VOLUME_USAGE", filesystem, blocks, _used, available, _capacity, mounted] => {
                mount.filesystem = (*filesystem).to_string();
                mount.blocks_kb = (*blocks).to_string();
                mount.available_kb = (*available).to_string();
                mount.mounted_on = (*mounted).to_string();
            }
            ["ERROR", message] => mount.error = Some((*message).to_string()),
            _ => {}
        }
    }
    mount
}

/// Mount `device` at `mount_point` on `target_name`, durably, and report.
pub async fn mount_volume(
    target_name: &str,
    device: &str,
    mount_point: &str,
    runner: &Runner,
) -> Result<(ComputeTarget, VolumeMount), DeployError> {
    validate_device(device).map_err(DeployError)?;
    validate_mount_point(mount_point).map_err(DeployError)?;
    let target = host_channel::canonical_target(target_name).await?;
    let program = PROGRAM
        .replace(DEVICE_MARK, &shlex_quote(device))
        .replace(MOUNT_POINT_MARK, &shlex_quote(mount_point))
        .replace("@FSTAB_DUMP_AND_PASS@", FSTAB_DUMP_AND_PASS)
        .replace("@FSTAB_TAG@", FSTAB_TAG);
    let output = host_channel::run_script(&target, &program, runner).await?;
    let mut mount = parse_output(&output.stdout);
    if mount.error.is_none() && output.code != 0 {
        mount.error = Some(format!(
            "the host program exited {} without a finding; stdout: {}",
            output.code,
            output.stdout.trim()
        ));
    }
    Ok((target, mount))
}

/// The `--json` report, in the shape every host operation reports.
pub fn to_report(target: &ComputeTarget, mount: &VolumeMount) -> Map<String, Value> {
    let mut report = host_channel::base_report(target);
    report.insert(
        "status".to_string(),
        json!(if mount.error.is_some() {
            "refused"
        } else if mount.mounted_now {
            "mounted"
        } else {
            "already_mounted"
        }),
    );
    report.insert(
        "volume".to_string(),
        json!({
            "device": mount.device,
            "fstype": mount.fstype,
            "uuid": mount.uuid,
            "size_bytes": mount.size_bytes,
            "fstab_line": mount.fstab_line,
            "fstab_written": mount.fstab_written,
            "mounted_now": mount.mounted_now,
            "mount_output": mount.mount_output,
            "filesystem": mount.filesystem,
            "blocks_kb": mount.blocks_kb,
            "available_kb": mount.available_kb,
            "mounted_on": mount.mounted_on,
        }),
    );
    report.insert("error".to_string(), json!(mount.error));
    report
}
