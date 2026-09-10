//! Putting a runner on a host: the exact installer program one registration
//! renders, the install that sends it, and the publisher secrets a desktop
//! release repository needs.

pub(super) mod install;
pub(super) mod installer;
pub(super) mod publisher;

pub use install::*;
pub use installer::*;
pub use publisher::*;
