//! What this host asks of its own hardware and of the canonical registry.
//!
//! Each probe answers a question the poll loop cannot answer out of its own
//! state: which target the registry declares for this hostname
//! ([`registry`]), whether the NVIDIA driver still enumerates a board
//! ([`cuda`]), and whether the two host-level declarations this agent
//! re-asserts — the board power cap ([`gpu_power`]) and the worker's
//! placement policy ([`placement`]) — currently hold.

pub mod cuda;
pub mod gpu_power;
pub mod placement;
pub mod registry;
