//! What the rest of the fleet reads about a slot: the status blob and the
//! heartbeat that fences the reaper, the redacted canonical output upload and
//! log tail, and the additive mirror to the caller's own object prefix.

use super::*;

mod heartbeat;
mod mirror;
mod output;

pub use heartbeat::*;
pub use mirror::*;
pub use output::*;
