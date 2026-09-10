//! Commands that reach a system outside the fleet: the control plane, the
//! egress path out of it, the mailbox it sends from, and the Vast.ai capacity
//! it rents.

pub mod control_plane;
pub mod egress;
pub mod mail;
pub mod vast;
