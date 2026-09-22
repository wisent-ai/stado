//! Folding the marker lines one host prints into a single reading.
//!
//! Split out of `reading.rs`, which had grown past the module line cap; the
//! shapes the fold fills in stay there.

use super::super::*;

/// Fold the marker lines of stdout into a reading.
pub fn parse_output(stdout: &str, policy_interval_seconds: Option<i64>) -> DiskReading {
    let mut reading = DiskReading::default();
    let mut memory_page_size: Option<i64> = None;
    let mut memory_pages: Vec<String> = Vec::new();
    let mut meminfo: Vec<String> = Vec::new();
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_CLONE_ROOT", path] => {
                reading.clone_root = Some((*path).to_string());
            }
            ["STADO_DISK", filesystem, blocks, used, available, capacity, mounted] => {
                reading.usage = Some(DiskUsage {
                    filesystem: (*filesystem).to_string(),
                    blocks_kb: (*blocks).to_string(),
                    used_kb: (*used).to_string(),
                    available_kb: (*available).to_string(),
                    capacity: (*capacity).to_string(),
                    mounted_on: (*mounted).to_string(),
                });
            }
            ["STADO_VOLUME", filesystem, blocks, used, available, capacity, mounted] => {
                reading.volumes.push(DiskUsage {
                    filesystem: (*filesystem).to_string(),
                    blocks_kb: (*blocks).to_string(),
                    used_kb: (*used).to_string(),
                    available_kb: (*available).to_string(),
                    capacity: (*capacity).to_string(),
                    mounted_on: (*mounted).to_string(),
                });
            }
            ["STADO_BLOCK_DEVICE", row] => reading.block_devices.push(parse_lsblk_pairs(row)),
            ["STADO_BLOCK_DEVICES_END", _] => reading.block_devices_read = true,
            ["STADO_CLEANUP_STATE", payload] => {
                reading.state = parse_state(payload, policy_interval_seconds);
            }
            ["STADO_CLEANUP_STATE_MISSING", path] => {
                reading.state = CleanupState {
                    path: Some((*path).to_string()),
                    ..CleanupState::default()
                };
            }
            ["STADO_CLEANUP_LOCK", pid, command] => {
                reading.lock_holders.push(LockHolder {
                    pid: (*pid).trim().to_string(),
                    command: (*command).trim().to_string(),
                });
            }
            // Printed whether or not anything held it, so "nobody is holding
            // the lock" is distinguishable from "this host could not be asked".
            ["STADO_MEMORY", "page_size", value] => {
                memory_page_size = value.split_whitespace().next().and_then(fold_int);
                reading.memory.page_size_bytes = memory_page_size;
            }
            ["STADO_MEMORY", "total_bytes", value] => {
                reading.memory.total_bytes = fold_int(value);
            }
            ["STADO_MEMORY", "swap", value] => {
                let (used, total) =
                    crate::providers::local::host_memory::reading::swapusage_bytes(value);
                reading.memory.swap_used_bytes = used;
                reading.memory.swap_total_bytes = total;
            }
            ["STADO_MEMORY_PAGES", row] => memory_pages.push((*row).to_string()),
            ["STADO_MEMINFO", row] => meminfo.push((*row).to_string()),
            ["STADO_MEMORY_STATE", payload] => {
                reading.memory_state = serde_json::from_str(payload).unwrap_or(Value::Null);
            }
            ["STADO_MEMORY_STATE_MISSING", _path] => {
                reading.memory_state = Value::Null;
            }
            ["STADO_CLEANUP_LOCK_END", path] => {
                reading.lock_read = true;
                reading.lock_path = Some((*path).to_string());
            }
            ["STADO_SNAPSHOT", name] => {
                reading.snapshots.supported = true;
                reading.snapshots.names.push((*name).to_string());
            }
            // The host has `tmutil` and listed what it has, which is how a Mac
            // with no snapshots at all is told apart from a host nobody could
            // ask.
            ["STADO_SNAPSHOT_END", _] => reading.snapshots.supported = true,
            ["STADO_DISK_ITEM", blocks, path] => {
                if let Ok(blocks_kb) = blocks.parse::<i64>() {
                    reading.inventory.push(DiskItem {
                        blocks_kb,
                        path: (*path).to_string(),
                    });
                }
            }
            ["STADO_BUILD_CACHE_ITEM", blocks, path] => {
                if let Ok(blocks_kb) = blocks.parse::<i64>() {
                    reading.tagged_build_caches.push(DiskItem {
                        blocks_kb,
                        path: (*path).to_string(),
                    });
                }
            }
            ["STADO_BUILD_CACHE_END", _] => reading.tagged_build_caches_read = true,
            ["STADO_CLONE_SUMMARY", path, total, hour, day] => {
                if let (Ok(total), Ok(older_than_hour), Ok(older_than_day)) = (
                    total.parse::<i64>(),
                    hour.parse::<i64>(),
                    day.parse::<i64>(),
                ) {
                    reading.clone_summaries.push(CloneSummary {
                        path: (*path).to_string(),
                        total,
                        older_than_hour,
                        older_than_day,
                    });
                }
            }
            _ => {}
        }
    }
    fold_memory(&mut reading, memory_page_size, &memory_pages, &meminfo);
    reading
}

/// `lsblk -P` prints `KEY="value"` pairs; the values are what the kernel
/// said, quotes and all, so an embedded quote arrives as `\"`.
pub fn parse_lsblk_pairs(row: &str) -> BlockDevice {
    let mut device = BlockDevice::default();
    let mut rest = row;
    while let Some(equals) = rest.find("=\"") {
        let key = rest[..equals].trim();
        let after = &rest[equals + 2..];
        let mut value = String::new();
        let mut chars = after.char_indices();
        let mut end = after.len();
        while let Some((index, ch)) = chars.next() {
            match ch {
                '\\' => {
                    if let Some((_, escaped)) = chars.next() {
                        value.push(escaped);
                    }
                }
                '"' => {
                    end = index + 1;
                    break;
                }
                other => value.push(other),
            }
        }
        match key {
            "NAME" => device.name = value,
            "SIZE" => device.size_bytes = value.parse().unwrap_or_default(),
            "TYPE" => device.kind = value,
            "FSTYPE" => device.fstype = value,
            "MOUNTPOINT" => device.mountpoint = value,
            "UUID" => device.uuid = value,
            "MODEL" => device.model = value,
            _ => {}
        }
        rest = &after[end.min(after.len())..];
    }
    device
}
