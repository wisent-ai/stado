//! What every fixed remote program shares: the prelude, the domain resolver,
//! the session read behind it, and the end states the probes assert.

mod remote_prelude;
mod session;
mod state;

pub use remote_prelude::*;
pub use session::*;
pub(crate) use state::*;
