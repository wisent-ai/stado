//! A host's resident memory, read from its own kernel and published as
//! readings.
//!
//! Nothing here acts on memory and nothing is declared per host: no
//! watermark, no repair, no refusal. A capacity publication carries the
//! numbers so an operator can read how much memory a host has left; whether a
//! host takes work is decided by its job admission and its disk, never by a
//! memory setting.

pub mod constants;
pub mod reading;

use serde_json::{Map, Value};

pub use reading::{read_host_memory, MemoryReading};

/// Bytes as GiB with one decimal, the shape every capacity figure is read in.
pub fn gigabytes(bytes: Option<i64>) -> Option<f64> {
    let gib = (constants::MIB * 1024) as f64;
    bytes.map(|bytes| (bytes as f64 / gib * 10.0).round() / 10.0)
}

/// The flat memory fields one capacity publication carries: what is
/// available, what is installed, and how much swap is in use. A field the
/// kernel did not answer is left out rather than published as zero.
pub fn publication_fields(reading: &MemoryReading) -> Map<String, Value> {
    let mut fields = Map::new();
    for (field, bytes) in [
        ("memory_available_gb", reading.available_bytes),
        ("memory_total_gb", reading.total_bytes),
    ] {
        if let Some(gb) = gigabytes(bytes) {
            fields.insert(field.into(), Value::from(gb));
        }
    }
    if let Some(pct) = reading.swap_used_pct() {
        fields.insert("memory_swap_used_pct".into(), Value::from(pct));
    }
    fields
}
