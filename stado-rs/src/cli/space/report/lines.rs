//! Two sections of the human-readable space report: the host's other
//! volumes, and its memory.
//!
//! Split out of `space/report/mod.rs`, which had grown past the module line
//! cap; the report itself stays there and calls both.

use serde_json::Value;

/// The mount point of the volume holding the declared `work_root`: the
/// longest mounted path the root sits under. Job trees and build caches live
/// there, so it is the fleet's volume as much as the home's is.
pub(super) fn work_root_mount(report: &Value) -> Option<String> {
    let root = report.get("work_root").and_then(Value::as_str)?;
    report
        .get("volumes")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|volume| volume.get("mounted_on").and_then(Value::as_str))
        .filter(|mount| {
            *mount == "/"
                || root == *mount
                || root
                    .strip_prefix(*mount)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .max_by_key(|mount| mount.len())
        .map(str::to_string)
}

/// Every other device-backed volume, and every disk the host has that
/// nothing mounted. The `disk:` line above is the volume holding the home;
/// the volume holding a declared work root is named as the fleet's too, and
/// a disk that is attached but unmounted is named as such rather than left out.
pub(super) fn print_volumes(report: &Value) {
    let fleet_volume = report
        .get("usage")
        .and_then(|usage| usage.get("filesystem"))
        .and_then(Value::as_str);
    let work_root = report.get("work_root").and_then(Value::as_str);
    let work_root_mount = report.get("work_root_mount").and_then(Value::as_str);
    // A volume field the probe did not report is said as such, not as the
    // word "unknown" sitting where a size or a mount point goes.
    let text = |value: &Value, key: &str| -> String {
        match value.get(key).and_then(Value::as_str) {
            Some(text) => text.to_string(),
            None => format!("<{key} not reported>"),
        }
    };
    let Some(volumes) = report.get("volumes").and_then(Value::as_array) else {
        println!("volumes: not reported; the probe carries no volumes list");
        return;
    };
    for volume in volumes.iter().filter(|volume| {
        fleet_volume.is_none() || volume.get("filesystem").and_then(Value::as_str) != fleet_volume
    }) {
        let whose = match (work_root, work_root_mount) {
            (Some(root), Some(mount))
                if volume.get("mounted_on").and_then(Value::as_str) == Some(mount) =>
            {
                format!("the fleet's work root {root} is here")
            }
            _ => "the fleet does not write here".to_string(),
        };
        println!(
            "volume: {} free KiB on {} mounted at {} ({}); {whose}",
            text(volume, "available_kb"),
            text(volume, "filesystem"),
            text(volume, "mounted_on"),
            text(volume, "capacity"),
        );
    }
    let devices = report.get("block_devices").unwrap_or(&Value::Null);
    if devices.get("read").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let Some(listed) = devices.get("devices").and_then(Value::as_array) else {
        println!("block devices: read, but the probe carries no devices list");
        return;
    };
    for device in listed
        .iter()
        .filter(|device| device.get("unmounted").and_then(Value::as_bool) == Some(true))
    {
        let size_gib = device
            .get("size_bytes")
            .and_then(Value::as_i64)
            .map_or_else(
                || "size not reported".to_string(),
                |bytes| format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0)),
            );
        let fstype = text(device, "fstype");
        println!(
            "attached, not mounted: /dev/{} {size_gib} {} ({}); nothing on this host can write to it until it is mounted",
            text(device, "name"),
            text(device, "model").trim(),
            if fstype.is_empty() || fstype == "unknown" { "no filesystem".to_string() } else { fstype },
        );
    }
}

/// The memory lines of the human-readable report: what the host has, and
/// what its paging looks like. Readings only — nothing is measured against
/// a watermark and nothing reclaims memory.
pub(super) fn print_memory(reading: &Value) {
    // A reading the host did not publish is said as not reported; the word
    // "unknown", and a page size of zero, stood in for measurements.
    let number = |key: &str| -> String {
        match reading.get(key).and_then(Value::as_i64) {
            Some(found) => found.to_string(),
            None => format!("<{key} not reported>"),
        }
    };
    const MIB: i64 = 1024 * 1024;
    let total_mb = match reading.get("total_bytes").and_then(Value::as_i64) {
        Some(bytes) => (bytes / MIB).to_string(),
        None => "<total_bytes not reported>".to_string(),
    };
    println!(
        "memory: {} MiB available of {total_mb} MiB, swap {}% used",
        number("available_mb"),
        number("swap_used_pct"),
    );
    // The compressor and the lifetime swapouts explain a host that has memory
    // left and still cannot answer a three-second probe.
    let compressor = reading.get("compressor_pages").and_then(Value::as_i64);
    let swapouts = reading.get("swapouts").and_then(Value::as_i64);
    if compressor.is_some() || swapouts.is_some() {
        let page = reading.get("page_size_bytes").and_then(Value::as_i64);
        let compressed = match (compressor, page) {
            (Some(pages), Some(page)) if page > 0 => {
                format!("{} MiB", pages.saturating_mul(page) / MIB)
            }
            (Some(pages), _) => format!("{pages} pages"),
            (None, _) => "<compressor_pages not reported>".to_string(),
        };
        println!(
            "memory paging: compressor holds {compressed}, {} since boot",
            match swapouts {
                Some(count) => format!("{count} swapout(s)"),
                None => "swapouts not reported".to_string(),
            },
        );
    }
}
