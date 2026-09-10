//! The last registry a reader could trust, and where that copy is kept when
//! the authority cannot be reached.

mod last_good;
mod last_good_store;

pub use last_good::*;
pub use last_good_store::*;
