//! What runs on a host that no declaration accounts for: the delivery roots,
//! the unowned processes, the undeclared units, and the one script that reads
//! every loaded label.

mod loaded_script;
mod process;
mod roots;
mod undeclared;

pub(crate) use loaded_script::*;
pub use process::*;
pub use roots::*;
pub use undeclared::*;
