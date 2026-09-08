//! The lifecycle bodies — restart, show, stop, probe, retire, and the
//! privileged daemon pair — plus the assembly that splices a body onto the
//! shared prelude.

mod assemble;
mod daemon;
mod restart;
mod stop;

pub use assemble::*;
pub(crate) use daemon::*;
pub(crate) use restart::*;
pub(crate) use stop::*;
