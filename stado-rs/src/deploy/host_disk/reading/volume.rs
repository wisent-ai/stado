//! One `df -Pk` row as a measured volume, or why it is not one.
//!
//! The host verdict, the disk report's coverage and the fleet-shape disk
//! check all judge the disk-full rule on the fleet volume's row; one reader
//! keeps them from disagreeing about a row whose figures are not numbers.

use serde_json::Value;

use super::super::DiskUsage;
use crate::providers::local::host_memory::constants::KIB;
use crate::providers::local::space_reclaim::rule::VolumeReading;

/// The fleet volume's size and free space in bytes, from `df -Pk`'s
/// 1024-byte blocks; a field that is not a block count, or a count past a
/// signed byte figure, is refused by name.
pub fn volume_of(usage: &DiskUsage) -> Result<VolumeReading, String> {
    let bytes = |field: &str, value: &str| -> Result<i64, String> {
        let blocks = value.parse::<i64>().map_err(|error| {
            format!("df row for {}: {field} {value:?} is not a block count: {error}", usage.mounted_on)
        })?;
        blocks.checked_mul(KIB).ok_or_else(|| {
            format!("df row for {}: {field} {blocks} blocks is more bytes than a signed count holds", usage.mounted_on)
        })
    };
    Ok(VolumeReading {
        total_bytes: bytes("blocks", &usage.blocks_kb)?,
        free_bytes: bytes("available", &usage.available_kb)?,
    })
}

/// The fleet volume of a `space report` document (its `usage` object, as
/// [`super::super::disk_report`] writes the row), read by [`volume_of`]; a
/// report without the row or one of its figures is refused naming what is
/// missing.
pub fn volume_of_report(report: &Value) -> Result<VolumeReading, String> {
    let usage = &report["usage"];
    let field = |name: &str| -> Result<String, String> {
        match &usage[name] {
            Value::String(text) => Ok(text.clone()),
            Value::Null => Err(format!("the disk report carries no usage.{name}: the host's df row was not read")),
            other => Err(format!("the disk report's usage.{name} is {other}, not df's text")),
        }
    };
    volume_of(&DiskUsage {
        filesystem: field("filesystem")?,
        blocks_kb: field("blocks_kb")?,
        used_kb: field("used_kb")?,
        available_kb: field("available_kb")?,
        capacity: field("capacity")?,
        mounted_on: field("mounted_on")?,
    })
}
