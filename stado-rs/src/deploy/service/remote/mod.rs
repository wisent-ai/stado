//! The approved channel: what the fixed remote programs report back, how they
//! are run, what operator data may be spliced into them, and the programs
//! themselves.

mod report;
mod run;
mod scripts;
mod validate;

pub use report::*;
pub use run::*;
pub use scripts::*;
pub use validate::*;
