//! The write side: one command per fixed remote program. Lifecycle, secret
//! and environment delivery, unit deployment, and the file and log reads.

mod deploy;
mod files;
mod lifecycle;
mod secrets;

pub use deploy::*;
pub use files::*;
pub use lifecycle::*;
pub use secrets::*;
