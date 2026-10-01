//! One exact label, addressed without a declaration: reaping, bootout,
//! persistent autostart, retiring a product's catalog predecessors and
//! giving them back when their replacement did not start, and the calling
//! account's posture.

mod autostart;
mod bootout;
mod posture;
mod predecessors;
mod reap;
mod reinstate;

pub use autostart::*;
pub use bootout::*;
pub use posture::*;
pub use predecessors::*;
pub use reap::*;
pub use reinstate::*;
