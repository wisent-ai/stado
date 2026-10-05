//! What the rest of the fleet reads about a slot: the status blob and the
//! heartbeat that fences the reaper, the redacted canonical output upload and
//! log tail, and the additive mirror to the caller's own object prefix.

use super::*;

mod heartbeat;
mod mirror;
mod output;
mod promise;

pub use heartbeat::*;
pub use mirror::*;
pub use output::*;
pub use promise::{lease_promise, record_renewal};
