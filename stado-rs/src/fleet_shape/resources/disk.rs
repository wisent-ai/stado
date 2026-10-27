//! Every managed host's volume against the disk-full rule.

use super::super::{Finding, DISK_CHECK};
use crate::deploy::{host_disk, Runner};
use crate::providers::local::space_reclaim::rule::DISK_FULL_PERCENT;
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
    // The one df-row reader the host verdict and the disk report use, so this
    // check cannot disagree with them about a row whose figures are not
    // numbers, and says which figure it could not read.
    let volume = match host_disk::volume_of_report(&report) {
        Ok(volume) => volume,
        Err(cause) => {
            out.push(Finding {
                check: DISK_CHECK,
                subject: target.name.clone(),
                declared: format!("the volume stays under {DISK_FULL_PERCENT}% used"),
                observed: cause,
                command: format!("stado space report {} --json", target.name),
            });
            return;
        }
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
