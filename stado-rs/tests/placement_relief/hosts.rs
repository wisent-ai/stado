//! The three publications every case here is written against: a host over its
//! watermark, the same host once the dip has passed, and a host with room.

use crate::support::{
    memory, LAPTOP_AVAILABLE_GB, LAPTOP_TOTAL_GB, MINI_AVAILABLE_GB, MINI_SWAP_PCT, MINI_TOTAL_GB,
};

/// The mini's own publication when a dip has passed: memory back above its
/// floor, swap still high, and its agent calling the pressure clear — the
/// exact reading the tick sampled on 2026-09-21 and settled on.
pub(crate) const MINI_CLEAR_AVAILABLE_GB: f64 = 2.5;
/// An age inside the stage's 900-second pressure window, and one past it.
pub(crate) const INSIDE_PRESSURE_WINDOW_SECONDS: i64 = 120;
pub(crate) const PAST_PRESSURE_WINDOW_SECONDS: i64 = 1200;

pub(crate) fn pressured_mini() -> serde_json::Value {
    memory(MINI_AVAILABLE_GB, MINI_TOTAL_GB, MINI_SWAP_PCT, true)
}

pub(crate) fn clear_mini() -> serde_json::Value {
    memory(MINI_CLEAR_AVAILABLE_GB, MINI_TOTAL_GB, MINI_SWAP_PCT, false)
}

pub(crate) fn roomy_laptop() -> serde_json::Value {
    memory(LAPTOP_AVAILABLE_GB, LAPTOP_TOTAL_GB, 0.0, false)
}
