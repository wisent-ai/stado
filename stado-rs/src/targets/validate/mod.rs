//! Everything a registry has to satisfy before it is written: the host's own
//! fields, and the document read as a whole.

mod basics;
mod disk;
mod document;
mod identity;
mod onboarding;

pub use basics::*;
pub use document::*;
// These three parts hold only crate-internal helpers, so their globs carry
// exactly that far.
pub(crate) use disk::*;
pub(crate) use identity::*;
pub(crate) use onboarding::*;
