mod previous;
mod removal;
mod rollback;

pub use removal::remove;
pub use rollback::rollback;
pub(in crate::install) use rollback::rollback_expected;
