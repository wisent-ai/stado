//! The registry document itself: parsing it, storing it, fetching it from the
//! authority, and the services each target declares inside it.

mod fetch;
mod parse;
mod service;
mod store;

pub use fetch::*;
pub use parse::*;
pub use service::*;
pub use store::*;
