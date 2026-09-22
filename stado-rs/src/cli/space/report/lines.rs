//! Two sections of the human-readable space report: the host's other
//! volumes, and its memory.
//!
//! Split out of `space/report/mod.rs`, which had grown past the module line
//! cap; the report itself stays there and calls both.

use serde_json::Value;

/// Every other device-backed volume, and every disk the host has that
/// nothing mounted. The `disk:` line above is the volume the fleet writes
/// to; a host that holds terabytes elsewhere says so here, and a disk that
/// is attached but unmounted is named as such rather than left out.
pub(super) fn print_volumes(report: &Value) {
    let fleet_volume = report
        .get("usage")
        .and_then(|usage| usage.get("filesystem"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text = |value: &Value, key: &str| -> String {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string()
    };
    for volume in report
        .get("volumes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|volume| volume.get("filesystem").and_then(Value::as_str) != Some(fleet_volume))
    {
        println!(
            "volume: {} free KiB on {} mounted at {} ({}); the fleet does not write here",
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
    for device in devices
        .get("devices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|device| device.get("unmounted").and_then(Value::as_bool) == Some(true))
    {
        let size_gib = device
            .get("size_bytes")
            .and_then(Value::as_i64)
            .map_or_else(
                || "unknown".to_string(),
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

/// The memory lines of the human-readable report.
///
/// Three facts, in the order an operator needs them: what the host has, what
/// it is measured against and whether it is over, and what the last pass
/// actually did. A number with no watermark beside it is what this report
/// printed before 2026-09-06, and it is why a host that could not give a
/// runtime its heap read as healthy.
pub(super) fn print_memory(memory: &Value) {
    let policy = memory
        .get("declaration")
        .and_then(|declaration| declaration.get("policy"))
        .unwrap_or(&Value::Null);
    let number = |value: &Value, key: &str| -> String {
        value
            .get(key)
            .and_then(Value::as_i64)
            .map_or_else(|| "unknown".to_string(), |found| found.to_string())
    };
    let reading = memory.get("reading").unwrap_or(&Value::Null);
    let mib = crate::providers::local::host_memory::constants::MIB;
    let total_mb = reading
        .get("total_bytes")
        .and_then(Value::as_i64)
        .map_or_else(|| "unknown".to_string(), |bytes| (bytes / mib).to_string());
    println!(
        "memory: {} MiB available of {total_mb} MiB, swap {}% used",
        number(reading, "available_mb"),
        number(reading, "swap_used_pct"),
    );
    // The compressor and the lifetime swapouts are read on every pass and
    // were printed nowhere. They are what explains a host that has memory
    // left and still cannot answer a three-second probe: on
    // charless-mac-mini on 2026-09-19 the line above read 4487 MiB
    // available and swap 71%, both inside their watermarks, while the
    // released Brama was quarantined for readiness twice in one hour.
    let compressor = reading.get("compressor_pages").and_then(Value::as_i64);
    let swapouts = reading.get("swapouts").and_then(Value::as_i64);
    if compressor.is_some() || swapouts.is_some() {
        let page = reading
            .get("page_size_bytes")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let compressed = match (compressor, page > 0) {
            (Some(pages), true) => format!("{} MiB", pages.saturating_mul(page) / mib),
            (Some(pages), false) => format!("{pages} pages"),
            (None, _) => "unknown".to_string(),
        };
        println!(
            "memory paging: compressor holds {compressed}, {} swapout(s) since boot",
            swapouts.map_or_else(|| "unknown".to_string(), |count| count.to_string()),
        );
    }
    println!(
        "memory watermark: mode {}, low {} MiB, target {} MiB, swap {}%{}",
        policy
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        number(policy, "low_free_mb"),
        number(policy, "target_free_mb"),
        number(policy, "high_swap_used_pct"),
        if memory
            .get("declaration")
            .and_then(|declaration| declaration.get("declared"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            ""
        } else {
            " (reporting default: this host declares none)"
        }
    );
    let last = memory.get("last_pass").unwrap_or(&Value::Null);
    let report = last.get("report").unwrap_or(&Value::Null);
    println!(
        "memory janitor: {} (writer {}, at {})",
        report
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("never_run"),
        report
            .get("writer")
            .and_then(Value::as_str)
            .unwrap_or("none"),
        report
            .get("started_at")
            .and_then(Value::as_str)
            .unwrap_or("never"),
    );
    if report.get("refuse_placement").and_then(Value::as_bool) == Some(true)
        && report.get("pressure_active").and_then(Value::as_bool) == Some(true)
    {
        println!(
            "memory placement: refused — this host publishes accepting_jobs=false with \
             admission_reason memory_pressure_active while it is over its watermark"
        );
    }
}
