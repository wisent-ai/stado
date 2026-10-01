//! `stado space volume unmount TARGET --mount-point PATH`: the inverse of
//! [`super::mount_volume`]. The filesystem is unmounted and the fstab line
//! `mount` wrote for it is removed, so the host stops bringing it up; the
//! data on the disk is not touched.
//!
//! Only a line carrying [`super::FSTAB_TAG`] is removed. A mount point that
//! an fstab line this command did not write declares is refused by name: that
//! line is someone else's decision. A busy filesystem is refused with
//! `umount`'s own sentence, and nothing in fstab changes. A mount point that
//! is neither mounted nor declared reports that and changes nothing, so a
//! repeated unmount is harmless.

use serde_json::{json, Map, Value};

use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

const MOUNT_POINT_MARK: &str = "@MOUNT_POINT@";
const TAG_MARK: &str = "@FSTAB_TAG@";

const PROGRAM: &str = r#"set -u
mount_point=@MOUNT_POINT@
tag='@FSTAB_TAG@'
if [ ! -r /proc/self/mounts ]; then
  printf 'ERROR\t%s\n' 'this host has no /proc/self/mounts; only a Linux host can be asked to unmount a volume'
  exit 1
fi
foreign=$(awk -v point="$mount_point" -v tag="$tag" '$1 !~ /^#/ && $2 == point && index($0, tag) == 0 { print; exit }' /etc/fstab 2>/dev/null)
if [ -n "$foreign" ]; then
  printf 'ERROR\t/etc/fstab declares %s with a line stado did not write (%s); remove that line by hand or leave the volume mounted\n' "$mount_point" "$foreign"
  exit 1
fi
unmounted_now=0
if awk -v point="$mount_point" '$2 == point { found = 1 } END { exit !found }' /proc/self/mounts; then
  if ! output=$(umount "$mount_point" 2>&1); then
    printf 'ERROR\tumount %s failed: %s\n' "$mount_point" "$output"
    exit 1
  fi
  unmounted_now=1
fi
printf 'STADO_VOLUME_UNMOUNTED\t%s\n' "$unmounted_now"
fstab_removed=0
if awk -v point="$mount_point" -v tag="$tag" '$2 == point && index($0, tag) > 0 { found = 1 } END { exit !found }' /etc/fstab; then
  cp -p /etc/fstab "/etc/fstab.before-stado-volume-$(date -u +%Y%m%d)" || { printf 'ERROR\tcannot back up /etc/fstab\n'; exit 1; }
  kept=$(awk -v point="$mount_point" -v tag="$tag" '!($2 == point && index($0, tag) > 0)' /etc/fstab) || { printf 'ERROR\tcannot read /etc/fstab\n'; exit 1; }
  printf '%s\n' "$kept" >/etc/fstab || { printf 'ERROR\tcannot write /etc/fstab\n'; exit 1; }
  fstab_removed=1
  if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload >/dev/null 2>&1 || printf 'ERROR\t%s\n' 'systemctl daemon-reload failed after the fstab line was removed'
  fi
fi
printf 'STADO_VOLUME_FSTAB_REMOVED\t%s\n' "$fstab_removed"
"#;

/// What the host did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VolumeUnmount {
    pub unmounted_now: bool,
    pub fstab_removed: bool,
    pub error: Option<String>,
}

fn parse_output(stdout: &str) -> VolumeUnmount {
    let mut unmount = VolumeUnmount::default();
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_VOLUME_UNMOUNTED", now] => unmount.unmounted_now = *now == "1",
            ["STADO_VOLUME_FSTAB_REMOVED", removed] => unmount.fstab_removed = *removed == "1",
            ["ERROR", message] => unmount.error = Some((*message).to_string()),
            _ => {}
        }
    }
    unmount
}

/// Unmount `mount_point` on `target_name` and withdraw the fstab line this
/// product wrote for it, and report.
pub async fn unmount_volume(
    target_name: &str,
    mount_point: &str,
    runner: &Runner,
) -> Result<(ComputeTarget, VolumeUnmount), DeployError> {
    super::validate_mount_point(mount_point).map_err(DeployError)?;
    let target = host_channel::canonical_target(target_name).await?;
    let program = PROGRAM
        .replace(MOUNT_POINT_MARK, &shlex_quote(mount_point))
        .replace(TAG_MARK, super::FSTAB_TAG);
    let output = host_channel::run_script(&target, &program, runner).await?;
    let mut unmount = parse_output(&output.stdout);
    if unmount.error.is_none() && output.code != 0 {
        unmount.error = Some(format!(
            "the host program exited {} without a finding; stdout: {}",
            output.code,
            output.stdout.trim()
        ));
    }
    Ok((target, unmount))
}

/// The `--json` report, in the shape every host operation reports.
pub fn to_report(
    target: &ComputeTarget,
    mount_point: &str,
    unmount: &VolumeUnmount,
) -> Map<String, Value> {
    let mut report = host_channel::base_report(target);
    let status = if unmount.error.is_some() {
        "refused"
    } else if unmount.unmounted_now || unmount.fstab_removed {
        "unmounted"
    } else {
        "not_mounted"
    };
    report.insert("status".to_string(), json!(status));
    report.insert(
        "volume".to_string(),
        json!({
            "mount_point": mount_point,
            "unmounted_now": unmount.unmounted_now,
            "fstab_removed": unmount.fstab_removed,
        }),
    );
    report.insert("error".to_string(), json!(unmount.error));
    report
}
