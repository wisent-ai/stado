//! The placement decision this host's memory declaration produces, and the
//! numbers behind it.
//!
//! The refusal itself has always been published: a host over its watermark
//! publishes `accepting_jobs: false` with `memory_pressure_active` as the
//! admission reason. What it never published were the numbers, while the disk
//! twin publishes all of them - free bytes, the low watermark, the target, the
//! policy mode and the janitor's whole last report.
//!
//! On 2026-09-10 that asymmetry cost a release. `skarbiec` could not build
//! `linux-amd64` because the only Linux builder was refusing placement, and
//! `stado host gates ubuntu-server-rtx-pro-6000` answered `accepting_jobs:
//! false` with 62.8 GiB of 123.0 GiB free RAM and not one word about which
//! watermark had been crossed: the reason sat in `diag.admission_reason` with
//! no measurement beside it, so nobody could tell whether memory or swap had
//! refused the host, or by how much.

use serde_json::{Map, Value};

use super::{
    last_report_in, over_published_watermark, persisted_watermark, refusal_reason,
    PublishedWatermark,
};
use crate::providers::local::host_memory::constants;
use crate::providers::local::host_memory::reading::{read_host_memory, MemoryReading};
/// What this host's own declaration says about taking new work right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlacementDecision {
    /// The admission reason to publish, or `None` when this host takes work.
    pub reason: Option<&'static str>,
    /// Whether the declaration refuses placement at all. A registry field,
    /// never inferred from a reading.
    pub refuse_placement: bool,
    /// The watermarks the reading was measured against, when this host has
    /// recorded a completed pass to publish them from.
    pub watermark: Option<PublishedWatermark>,
    /// The live reading the decision was made on.
    pub reading: MemoryReading,
}

impl PlacementDecision {
    /// The flat memory fields a capacity publication carries, beside the
    /// janitor's own last report under `memory_reclaim`.
    ///
    /// Published whether or not this host refuses work: a margin nobody can
    /// read until it is crossed is a margin nobody can act on, and the disk
    /// half has always published `free_disk_gb` on a healthy host too.
    pub fn diagnostics(&self) -> Map<String, Value> {
        let mut diag = Map::new();
        if let Some(reason) = self.reason {
            diag.insert("memory_pressure_active".into(), Value::Bool(true));
            diag.insert("admission_reason".into(), Value::String(reason.to_string()));
        }
        diag.insert(
            "memory_refuse_placement".into(),
            Value::Bool(self.refuse_placement),
        );
        for (field, bytes) in [
            ("memory_available_gb", self.reading.available_bytes),
            ("memory_total_gb", self.reading.total_bytes),
            (
                "memory_low_watermark_gb",
                self.watermark.map(|watermark| watermark.low_bytes),
            ),
        ] {
            if let Some(gb) = gigabytes(bytes) {
                diag.insert(field.into(), Value::from(gb));
            }
        }
        if let Some(pct) = self.reading.swap_used_pct() {
            diag.insert("memory_swap_used_pct".into(), Value::from(pct));
        }
        if let Some(watermark) = self.watermark {
            diag.insert(
                "memory_swap_high_watermark_pct".into(),
                Value::from(watermark.high_swap_used_pct),
            );
        }
        let report = last_report_in(&crate::config_file::expand_tilde("~"));
        if !report.is_null() {
            diag.insert("memory_reclaim".into(), report);
        }
        diag
    }
}

/// Bytes as GiB with one decimal, the shape every capacity figure is read in.
fn gigabytes(bytes: Option<i64>) -> Option<f64> {
    let gib = (constants::MIB * 1024) as f64;
    bytes.map(|bytes| (bytes as f64 / gib * 10.0).round() / 10.0)
}

/// This host's placement decision, made once per publication.
///
/// One reading, one verdict, one document: the reason and the numbers beside
/// it come from the same measurement, so a published refusal can never name a
/// margin the host was not actually at.
pub fn placement_decision() -> PlacementDecision {
    let watermark = persisted_watermark();
    let reading = read_host_memory();
    let refuse_placement = watermark.is_some_and(|watermark| watermark.refuse_placement);
    let over = watermark.and_then(|watermark| over_published_watermark(watermark, &reading));
    PlacementDecision {
        reason: refusal_reason(refuse_placement, over),
        refuse_placement,
        watermark,
        reading,
    }
}
