//! The disk gate of one Cargo operation: the volume the compile writes to.
//!
//! Cargo writes its target into the host's build cache ([`super::script`]),
//! which lives under the registry's declared work root when the host has
//! one. Measuring the host's home volume instead refused every build on a
//! host whose root volume is past the disk-full threshold while its work
//! root sat on a volume with terabytes free, and admitted builds onto a full
//! work-root volume. The volume measured is the one holding the build cache,
//! read with `df` at its nearest existing directory, so a cache not created
//! yet is measured where it will be created.

use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// What separates the fields of the one line the measurement prints.
const FIELD: char = '\t';

/// The volume holding `target`'s build cache, as one `df -P` read it.
pub(super) struct Measured {
    pub(super) used_percent: f64,
    pub(super) free_bytes: u64,
    mount: String,
    path: String,
}

/// Refuse a compile on `target` when the volume its build cache is on is at
/// or past the disk-full threshold, naming the volume and its mount; the
/// reading admitted is returned, its free bytes the receipt's `before`.
pub(super) async fn admit(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Measured, DeployError> {
    let measured = measure(target, runner).await?;
    let threshold = crate::providers::local::space_reclaim::rule::DISK_FULL_PERCENT;
    if measured.used_percent >= f64::from(threshold) {
        return Err(DeployError::unreachable(format!(
            "{}: the volume at {} holding its build cache ({}) is {:.1}% used, at the \
             {threshold}% disk-full threshold; a compile would fill it while its janitor deletes \
             everything the fleet put there (`stado space report {}`)",
            target.name, measured.mount, measured.path, measured.used_percent, target.name
        )));
    }
    Ok(measured)
}

/// Read the volume holding `target`'s build cache.
pub(super) async fn measure(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Measured, DeployError> {
    let root = super::script::cache_root(target);
    // POSIX `df -P` prints one header and one line per filesystem: its
    // device, 1024-blocks, used, available, capacity and mount point
    // (https://pubs.opengroup.org/onlinepubs/9799919799/utilities/df.html).
    let script = format!(
        "set -eu\np={root}\nwhile [ ! -e \"$p\" ]; do p=$(dirname -- \"$p\"); done\n\
         df -Pk -- \"$p\" | awk 'NR==2 {{ print $3 \"\\t\" $4 \"\\t\" $6 }}'\nprintf '%s\\n' \"$p\"\n"
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.succeeded() {
        return Err(DeployError::unreachable(format!(
            "{}: the volume of its build cache could not be measured: {}",
            target.name,
            host_channel::whole_failure(&output)
        )));
    }
    let mut lines = output.stdout.lines();
    let (line, measured) = (
        lines.next().unwrap_or_default(),
        lines.next().unwrap_or_default(),
    );
    let fields: Vec<&str> = line.split(FIELD).collect();
    let [used, available, mount] = fields.as_slice() else {
        return Err(DeployError::unreachable(format!(
            "{}: measuring its build cache's volume answered {:?}, not used, available and mount",
            target.name, output.stdout
        )));
    };
    let parse = |field: &str, what: &str| {
        field.parse::<u64>().map_err(|error| {
            DeployError::unreachable(format!(
                "{}: its build cache's volume states {what} {field:?}: {error}",
                target.name
            ))
        })
    };
    let (used, available) = (
        parse(used, "used blocks")?,
        parse(available, "available blocks")?,
    );
    let total = used.saturating_add(available);
    if total == 0 {
        return Err(DeployError::unreachable(format!(
            "{}: the volume at {mount} holding its build cache ({measured}) states no capacity",
            target.name
        )));
    }
    Ok(Measured {
        used_percent: used as f64 * 100.0 / total as f64,
        // `df -Pk` counts in 1024-byte blocks.
        free_bytes: available.saturating_mul(1024),
        mount: (*mount).to_string(),
        path: measured.to_string(),
    })
}
