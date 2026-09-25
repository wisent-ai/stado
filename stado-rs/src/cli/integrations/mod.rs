//! Commands that reach a system outside the fleet: the control plane, the
//! egress path out of it, and the Vast.ai capacity it rents.

pub mod control_plane;
pub mod egress;
pub(crate) mod runtime;
pub mod vast;
