//! What the host's accelerators are doing, beyond how much memory is free:
//! which process holds each one, whether that process is a Stado job, and
//! how much of the used memory nobody the fleet knows about accounts for.
//!
//! On 2026-09-17 the RTX host published `VRAM 0/95 GiB free` with zero
//! Stado jobs, and nothing in the fleet could say what held the card. The
//! agent already reads per-process usage to size jobs; `holders` turns the
//! same driver answer into a published list.

pub mod holders;

pub use holders::{accelerator_holders_line, AcceleratorHolder, AcceleratorHolders};
