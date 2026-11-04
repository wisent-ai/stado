//! Commands that reach a system outside the fleet: the control plane, the
//! egress path out of it, the Vast.ai capacity it rents, the other Stado
//! fleets it trades with, and the networks whose own node a host runs.

pub mod egress;
pub(crate) mod node;
pub(crate) mod peer;
pub(crate) mod runtime;
pub mod vast;
