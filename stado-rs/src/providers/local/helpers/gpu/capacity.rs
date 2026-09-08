//! Which accelerator tiers this host's VRAM satisfies, and how many slots of
//! each tier fit on it.
//!
//! One table answers both questions: the GCP sizing tiers. A tier this host
//! clears is an accelerator name it may be routed work for, and the count of
//! that tier per card is what the capacity document broadcasts to the fleet.

use std::collections::BTreeMap;

use crate::catalog::GPU_SIZING;

/// Every GCP gpu_type whose required VRAM tier <= local VRAM.
/// Python `_compat_accel_types`.
pub fn compat_accel_types(local_vram_gb: i64) -> Vec<String> {
    let mut accels: Vec<String> = Vec::new();
    if let Some(sizing) = GPU_SIZING.get(crate::capabilities::ProviderId::Gcp.as_str()) {
        // BTreeMap iterates tiers ascending, matching Python's sorted(...).
        for (tier, (_, accel)) in sizing {
            if local_vram_gb >= *tier && !accel.is_empty() && !accels.iter().any(|a| a == accel) {
                accels.push((*accel).to_string());
            }
        }
    }
    accels
}

/// Slot-shaped capacity broadcast for back-compat schedulers.
/// Python `_build_capacity_dict`.
pub fn build_capacity_dict(
    gpu_type: &str,
    free_vram_gb: i64,
    total_vram_gb: i64,
) -> BTreeMap<String, i64> {
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    if gpu_type.is_empty() || gpu_type == "cpu" || free_vram_gb <= 0 {
        return out;
    }
    if let Some(sizing) = GPU_SIZING.get(crate::capabilities::ProviderId::Gcp.as_str()) {
        for (tier, (_, accel)) in sizing {
            if total_vram_gb >= *tier && !accel.is_empty() {
                let n = (free_vram_gb / (*tier).max(1)).max(0);
                if n > 0 {
                    let entry = out.entry((*accel).to_string()).or_insert(0);
                    *entry = (*entry).max(n);
                }
            }
        }
    }
    if !out.contains_key(gpu_type) {
        out.insert(gpu_type.to_string(), 1);
    }
    out
}

/// The same broadcast for a host with more than one card: how many slots of
/// each tier fit across all of them, and one entry for this host's own
/// gpu_type.
///
/// A tier count is summed per card, never derived from a pooled total. Two
/// 95 GiB boards do not hold a 190 GiB model, and a card something else is busy
/// on does not reduce what its neighbour can take -- the single-pool answer was
/// wrong in both directions at once.
pub fn build_capacity_dict_per_card(
    gpu_type: &str,
    free_vram_gb_per_card: &[i64],
    total_vram_gb: i64,
) -> BTreeMap<String, i64> {
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    if gpu_type.is_empty() || gpu_type == "cpu" {
        return out;
    }
    if free_vram_gb_per_card.iter().all(|free| *free <= 0) {
        return out;
    }
    if let Some(sizing) = GPU_SIZING.get(crate::capabilities::ProviderId::Gcp.as_str()) {
        for (tier, (_, accel)) in sizing {
            if total_vram_gb < *tier || accel.is_empty() {
                continue;
            }
            let n: i64 = free_vram_gb_per_card
                .iter()
                .map(|free| (free / (*tier).max(1)).max(0))
                .sum();
            if n > 0 {
                let entry = out.entry((*accel).to_string()).or_insert(0);
                *entry = (*entry).max(n);
            }
        }
    }
    if !out.contains_key(gpu_type) {
        let usable = free_vram_gb_per_card
            .iter()
            .filter(|free| **free > 0)
            .count() as i64;
        out.insert(gpu_type.to_string(), usable.max(1));
    }
    out
}
