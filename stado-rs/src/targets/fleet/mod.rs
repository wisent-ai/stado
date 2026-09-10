//! What the fleet may do with a target: the capabilities it carries, the
//! coordinator it answers to, where work may be placed, and the policies that
//! decide both.

mod capabilities;
mod coordinator;
mod placement;
mod policies;

pub use capabilities::*;
pub use coordinator::*;
pub use placement::*;
pub use policies::*;
