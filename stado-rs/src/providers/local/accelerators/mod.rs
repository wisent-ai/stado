//! What the host's accelerators are doing, beyond how much memory is free:
//! which process holds each one, whether that process is a Stado job, and
//! how much of the used memory nobody the fleet knows about accounts for.
//!
//! A GPU host can publish no free VRAM with zero Stado jobs, and nothing
//! in the fleet can then say what holds the card. The
//! agent already reads per-process usage to size jobs; `holders` turns the
//! same driver answer into a published list.

pub mod holders;

pub use holders::{accelerator_holders_line, AcceleratorHolder, AcceleratorHolders};
