//! Serving, publishing and signing releases read the vault through Stado's
//! Skarbiec identity.

mod publishers;
mod serving;

pub use publishers::*;
pub use serving::*;
