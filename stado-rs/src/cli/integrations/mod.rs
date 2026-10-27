//! Commands that reach a system outside the fleet: the control plane, the
//! egress path out of it, the Vast.ai capacity it rents, and the other Stado
//! fleets it trades with.

pub mod egress;
pub(crate) mod peer;
pub(crate) mod runtime;
pub mod vast;
