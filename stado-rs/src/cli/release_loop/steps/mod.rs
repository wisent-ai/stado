//! The steps the delivery loop takes a commit through: what Oko says it
//! carries, the version it is released under, the release itself and the
//! record of it, each with the programs it runs.

mod carried;
mod deliver;
pub(super) mod process;
mod version;

pub use carried::{open_repairs, unplaced, Unplaced};
pub use deliver::{pass, Mode};
