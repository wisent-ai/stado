//! Every managed host's volume against the disk-full rule.

use super::super::{Finding, DISK_CHECK};
use crate::deploy::{host_disk, Runner};
use crate::providers::local::disk_cleanup::rule::{VolumeReading, DISK_FULL_PERCENT};
use crate::targets::ComputeTarget;

/// A finding when a host's volume is at the disk-full threshold: its janitor
/// is deleting everything the fleet put there and it refuses work meanwhile.
pub(in crate::fleet_shape) async fn disk_headroom(
    target: &ComputeTarget,
    runner: &Runner,
    out: &mut Vec<Finding>,
) {
    let report = match host_disk::disk_host(&target.name, runner).await {
        Ok(report) => report,
        Err(error) => {
            out.push(Finding {
                check: DISK_CHECK,
                subject: target.name.clone(),
                declared: "the host answers df".to_string(),
                observed: format!("disk read failed: {error}"),
                command: format!("stado space report {}", target.name),
            });
            return;
        }
    };
    let kib = |key: &str| {
        report["usage"][key]
            .as_str()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .and_then(|value| value.checked_mul(1024))
    };
    let (Some(total_bytes), Some(free_bytes)) = (kib("blocks_kb"), kib("available_kb")) else {
        out.push(Finding {
            check: DISK_CHECK,
            subject: target.name.clone(),
            declared: format!("the volume stays under {DISK_FULL_PERCENT}% used"),
            observed: "df answered without its size and available columns".to_string(),
            command: format!("stado space report {} --json", target.name),
        });
        return;
    };
    let volume = VolumeReading {
        total_bytes,
        free_bytes,
    };
    if volume.full() {
        out.push(Finding {
            check: DISK_CHECK,
            subject: target.name.clone(),
            declared: format!("the volume stays under {DISK_FULL_PERCENT}% used"),
            observed: format!(
                "{:.1}% used, so the janitor deletes everything the fleet put there and the host refuses work",
                volume.used_percent()
            ),
            command: format!("stado space report {}", target.name),
        });
    }
}
