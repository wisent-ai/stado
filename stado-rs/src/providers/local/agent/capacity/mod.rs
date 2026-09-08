//! What the tick measures, what it broadcasts, and what it gives up.
//!
//! [`snapshot`] turns the host's live resources into the one capacity document
//! the fleet reads; [`yielding`] decides which running slot makes room for a
//! higher-priority queued job; [`inference`] is the reservation this host may
//! be holding a GPU for; and [`release`] is the handoff that ends the process
//! so a newer installed image can start.

pub mod inference;
pub mod release;
pub mod snapshot;
pub mod yielding;
