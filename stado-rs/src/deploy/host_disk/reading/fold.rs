//! Folding the marker lines one host prints into a single reading.
//!
//! Split out of `reading.rs`, which had grown past the module line cap; the
//! shapes the fold fills in stay there.

use super::super::*;

/// Fold the marker lines of stdout into a reading.
pub fn parse_output(stdout: &str) -> DiskReading {
    let mut reading = DiskReading::default();
    let mut memory = MemoryMarkers::default();
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_CLONE_ROOT", path] => {
                reading.clone_root = Some((*path).to_string());
            }
            ["STADO_DISK", filesystem, blocks, used, available, capacity, mounted] => {
                let row = DiskUsage {
                    filesystem: (*filesystem).to_string(),
                    blocks_kb: (*blocks).to_string(),
                    used_kb: (*used).to_string(),
                    available_kb: (*available).to_string(),
                    capacity: (*capacity).to_string(),
                    mounted_on: (*mounted).to_string(),
                };
                // One row per volume the fleet writes to (the home's and the
                // declared work root's); the one with less room before the
                // threshold is the reading, as the janitor's rule judges it.
                let tighter = match (&reading.usage, headroom_kb(&row)) {
                    (None, Ok(_)) => true,
                    // Kept, as the only row so far, and named as unmeasured.
                    (None, Err(cause)) => {
                        reading.unread.push(cause);
                        true
                    }
                    (Some(_), Err(cause)) => {
                        reading.unread.push(cause);
                        false
                    }
                    (Some(kept), Ok(new)) => match headroom_kb(kept) {
                        Ok(old) => new < old,
                        // The kept row's cause was recorded when it was kept.
                        Err(_kept_unmeasured) => true,
                    },
                };
                if tighter {
                    reading.usage = Some(row);
                }
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
            ["STADO_BLOCK_DEVICE", row] => {
                let device = parse_lsblk_pairs(row);
                if let Some(cause) = &device.size_unread {
                    reading.unread.push(format!("block device {}: {cause}", device.name));
                }
                reading.block_devices.push(device);
            }
            ["STADO_BLOCK_DEVICES_END", _] => reading.block_devices_read = true,
            ["STADO_CLEANUP_STATE", payload] => {
                reading.state = parse_state(payload);
            }
            ["STADO_CLEANUP_STATE_MISSING", path] => {
                reading.state = CleanupState {
                    path: Some((*path).to_string()),
                    ..CleanupState::default()
                };
            }
            ["STADO_LIVE_STADO", pids] => {
                let mut alive = Vec::new();
                for pid in pids.split_whitespace() {
                    match pid.parse::<u32>() {
                        Ok(pid) => alive.push(pid),
                        Err(error) => reading
                            .unread
                            .push(format!("live stado process {pid:?} is not a pid: {error}")),
                    }
                }
                reading.live_stado_pids = Some(alive);
            }
            ["STADO_LIVE_STADO"] => reading.live_stado_pids = Some(Vec::new()),
            ["STADO_CLEANUP_LOCK", pid, command] => {
                reading.lock_holders.push(LockHolder {
                    pid: (*pid).trim().to_string(),
                    command: (*command).trim().to_string(),
                });
            }
            fields if memory.take(fields) => {}
            // Printed whether or not anything held it, so "nobody is holding
            // the lock" is distinguishable from "this host could not be asked".
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
            ["STADO_DISK_ITEM", blocks, path] => match blocks.parse::<i64>() {
                Ok(blocks_kb) => reading.inventory.push(DiskItem {
                    blocks_kb,
                    path: (*path).to_string(),
                }),
                Err(error) => reading
                    .unread
                    .push(format!("directory {path}: size {blocks:?} is not a block count: {error}")),
            },
            ["STADO_BUILD_CACHE_ITEM", blocks, path] => match blocks.parse::<i64>() {
                Ok(blocks_kb) => reading.tagged_build_caches.push(DiskItem {
                    blocks_kb,
                    path: (*path).to_string(),
                }),
                Err(error) => reading
                    .unread
                    .push(format!("build cache {path}: size {blocks:?} is not a block count: {error}")),
            },
            ["STADO_BUILD_CACHE_END", _] => reading.tagged_build_caches_read = true,
            ["STADO_CLONE_SUMMARY", path, total, hour, day] => {
                let count = |name: &str, value: &str| {
                    value
                        .parse::<i64>()
                        .map_err(|error| format!("clone summary {path}: {name} {value:?} is not a count: {error}"))
                };
                let counts = count("total", total).and_then(|total| {
                    Ok((total, count("older than an hour", hour)?, count("older than a day", day)?))
                });
                match counts {
                    Ok((total, older_than_hour, older_than_day)) => {
                        reading.clone_summaries.push(CloneSummary {
                            path: (*path).to_string(),
                            total,
                            older_than_hour,
                            older_than_day,
                        })
                    }
                    Err(cause) => reading.unread.push(cause),
                }
            }
            _ => {}
        }
    }
    reading.memory = fold_memory(memory);
    reading
}

/// KiB that may still be written to the volume of `usage` before the
/// disk-full rule's threshold, by the rule's own arithmetic, or why a row
/// whose sizes are not numbers has none.
fn headroom_kb(usage: &DiskUsage) -> Result<i64, String> {
    let count = |name: &str, value: &str| {
        value.parse::<i64>().map_err(|error| {
            format!("df row for {}: {name} {value:?} is not a block count: {error}", usage.mounted_on)
        })
    };
    let total = count("blocks", &usage.blocks_kb)?;
    let free = count("available", &usage.available_kb)?;
    Ok(crate::providers::local::space_reclaim::rule::headroom_bytes(total, free))
}

/// `lsblk -P` prints `KEY="value"` pairs; the values are what the kernel
/// said, quotes and all, so an embedded quote arrives as `\"`.
pub fn parse_lsblk_pairs(row: &str) -> BlockDevice {
    const OPEN: &str = "=\"";
    let mut device = BlockDevice::default();
    let mut rest = row;
    while let Some(equals) = rest.find(OPEN) {
        let key = rest[..equals].trim();
        let after = &rest[equals + OPEN.len()..];
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
                    end = index + ch.len_utf8();
                    break;
                }
                other => value.push(other),
            }
        }
        match key {
            "NAME" => device.name = value,
            "SIZE" if value.is_empty() => {}
            "SIZE" => match value.parse::<i64>() {
                Ok(bytes) => device.size_bytes = Some(bytes),
                Err(error) => {
                    device.size_unread = Some(format!("lsblk printed the size {value:?}, which is not a byte count: {error}"))
                }
            },
            "TYPE" => {
                device.device_type = DeviceType::of(&value);
                device.kind = value;
            }
            "FSTYPE" => {
                device.signature = Signature::of(&value);
                device.fstype = value;
            }
            "MOUNTPOINT" => device.mountpoint = value,
            "UUID" => device.uuid = value,
            "MODEL" => device.model = value,
            _ => {}
        }
        rest = &after[end..];
    }
    device
}
