//! Documents: the registry array this module mutates, and the plist and
//! systemd unit texts it parses and redacts.

mod plist;
mod redact;
mod registry;
mod systemd;

pub use plist::*;
pub use redact::*;
pub use registry::*;
pub use systemd::*;
