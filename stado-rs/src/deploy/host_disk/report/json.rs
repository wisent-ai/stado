//! Writing one host's disk reading out as the JSON report `stado space
//! report` prints.
//!
//! Split out of `report.rs`, which had grown past the module line cap; the
//! state document and the reading itself stay there.

use super::super::*;

fn usage_json(usage: &DiskUsage) -> Value {
    json!({
        "filesystem": usage.filesystem,
        "blocks_kb": usage.blocks_kb,
        "used_kb": usage.used_kb,
        "available_kb": usage.available_kb,
        "capacity": usage.capacity,
        "mounted_on": usage.mounted_on,
    })
}

/// The reading as the `--json` report, in `host reboot`'s report shape.
pub fn to_report(target: &ComputeTarget, reading: &DiskReading) -> Map<String, Value> {
    let mut report = host_channel::base_report(target);
    report.insert(
        "usage".to_string(),
        reading.usage.as_ref().map_or(Value::Null, usage_json),
    );
    // Where the rest of the host's storage is. `usage` above is the one
    // volume the fleet writes to; a host can hold terabytes on another
    // mount, or on a disk nothing has mounted, and neither shows in `usage`.
    report.insert(
        "volumes".to_string(),
        Value::Array(reading.volumes.iter().map(usage_json).collect()),
    );
    report.insert(
        "block_devices".to_string(),
        json!({
            "read": reading.block_devices_read,
            "devices": reading.block_devices.iter().map(|device| json!({
                "name": device.name,
                "size_bytes": device.size_bytes,
                "type": device.kind,
                "fstype": device.fstype,
                "mountpoint": device.mountpoint,
                "uuid": device.uuid,
                "model": device.model,
                "unmounted": device.unmounted_among(&reading.block_devices),
            })).collect::<Vec<Value>>(),
        }),
    );
    // Beside the disk, because a host that cannot allocate and a host that
    // cannot write fail in the same commands and are repaired differently.
    // `free_kb` and `swap` stay as they were spelled so a reader written
    // against the first version of this report keeps working; everything a
    // watermark is measured against is under `memory_reclaim`.
    report.insert(
        "memory".to_string(),
        json!({
            "free_kb": available_kb(&reading.memory),
            "swap": swap_line(&reading.memory),
        }),
    );
    report.insert("memory_reclaim".to_string(), memory_report(target, reading));
    // The registry policy verbatim — same struct the janitor resolves, so
    // the operator is reading the declaration the host actually obeys.
    report.insert(
        "policy".to_string(),
        target
            .disk_cleanup
            .as_ref()
            .and_then(|policy| serde_json::to_value(policy).ok())
            .unwrap_or(Value::Null),
    );
    let state = &reading.state;
    report.insert(
        "cleanup_state".to_string(),
        json!({
            "present": state.present,
            "path": state.path,
            "last_pass_at": state.last_pass_at,
            "last_success_at": state.last_success_at,
            "last_prevented_at": state.last_prevented_at,
            "outcome": state.outcome,
            "writer": state.writer,
            "writer_version": state.writer_version,
            "writer_pid": state.writer_pid,
            "free_bytes_before": state.free_bytes_before,
            "free_bytes_after": state.free_bytes_after,
            "freed_bytes": state.freed_bytes,
            "next_pass_at": state.next_pass_at,
            "low_bytes": state.low_bytes,
            "error": state.error,
            "report": state.report,
        }),
    );
    // The other half of every `lock_busy` and `cleanup_in_progress` an
    // operator has ever read: which process is holding the run lock.
    report.insert(
        "cleanup_lock".to_string(),
        json!({
            "read": reading.lock_read,
            "path": reading.lock_path,
            "held": !reading.lock_holders.is_empty(),
            "holders": reading
                .lock_holders
                .iter()
                .map(|holder| json!({"pid": holder.pid, "command": holder.command}))
                .collect::<Vec<Value>>(),
        }),
    );
    // Reported next to the usage it does not appear in: `size_bytes` is
    // deliberately absent rather than null, because macOS states no size and a
    // key an operator could read as "zero" is worse than a key that is not
    // there. `reclaimable_by_stado` is the finding.
    let snapshots = &reading.snapshots;
    report.insert(
        "local_snapshots".to_string(),
        json!({
            "supported": snapshots.supported,
            "count": snapshots.names.len(),
            "names": snapshots.names,
            "reclaimable_by_stado": snapshots.names.iter().any(|name| {
                name.starts_with("com.apple.TimeMachine.") && name.ends_with(".local")
            }),
        }),
    );
    report.insert(
        "inventory".to_string(),
        Value::Array(
            reading
                .inventory
                .iter()
                .chain(outermost_build_caches(reading).iter())
                .map(|item| {
                    json!({
                        "path": item.path,
                        "bytes": item.blocks_kb.saturating_mul(1024),
                        "size_gb": gib_from_blocks(item.blocks_kb as f64),
                    })
                })
                .collect(),
        ),
    );
    // Also on its own, because "what the host holds in build output" is a
    // question with an answer, and reading it back out of the merged
    // inventory means guessing which rows came from the census.
    report.insert(
        "tagged_build_output".to_string(),
        Value::Array(
            outermost_build_caches(reading)
                .iter()
                .map(|item| {
                    json!({
                        "path": item.path,
                        "bytes": item.blocks_kb.saturating_mul(1024),
                    })
                })
                .collect(),
        ),
    );
    report.insert(
        "tagged_build_output_read".to_string(),
        json!(reading.tagged_build_caches_read),
    );
    report.insert("chromium_clone_root".to_string(), json!(reading.clone_root));
    report.insert(
        "chromium_clones".to_string(),
        Value::Array(
            reading
                .clone_summaries
                .iter()
                .map(|summary| {
                    json!({
                        "path": summary.path,
                        "total": summary.total,
                        "older_than_hour": summary.older_than_hour,
                        "older_than_day": summary.older_than_day,
                    })
                })
                .collect(),
        ),
    );
    report
}

/// The census rows worth keeping: every tagged tree that is not inside
/// another tagged tree, and that the depth-bounded inventory did not already
/// name.
///
/// `du -sxk` on a tagged directory counts everything below it, so a tagged
/// tree nested in another would be charged to the disk twice and the coverage
/// partition would report more unswept bytes than the host holds.
fn outermost_build_caches(reading: &DiskReading) -> Vec<crate::deploy::host_disk::DiskItem> {
    let mut rows = reading.tagged_build_caches.clone();
    rows.sort_by(|left, right| left.path.cmp(&right.path));
    let mut kept: Vec<crate::deploy::host_disk::DiskItem> = Vec::new();
    for row in rows {
        let nested = kept
            .last()
            .is_some_and(|previous| row.path.starts_with(&format!("{}/", previous.path)));
        if nested {
            continue;
        }
        if reading
            .inventory
            .iter()
            .any(|measured| measured.path == row.path)
        {
            continue;
        }
        kept.push(row);
    }
    kept
}
