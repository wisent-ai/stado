//! Commands that reach a system outside the fleet: the control plane, the
//! egress path out of it, the marketplaces a host is offered on (each an
//! adapter of [`market::MarketAdapter`]: Vast.ai and other Stado fleets),
//! and the networks whose own node a host runs.

pub mod egress;
pub(crate) mod market;
pub(crate) mod node;
pub(crate) mod peer;
pub(crate) mod runtime;
pub mod vast;
