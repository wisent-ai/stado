//! Declarative, registry-backed service placement groups.
//!
//! A profile names the logical services that move together, their lifecycle
//! owner on each eligible host, the state files that travel, health probes,
//! and Stado-managed routing units whose desired state depends on the selected
//! destination.
//! Runtime transactions are recorded in the same compare-and-swapped registry
//! document so two operators cannot relocate services concurrently.

mod document;
mod model;
mod runtime;
mod validate;

pub use document::{profiles, root_object, transactions};
pub use model::{
    ManagedPlacementUnit, PlacementHost, PlacementLifecycle, PlacementProbe, PlacementProfile,
    PlacementRoute, PlacementState, PlacementTransaction, PlacementUnit,
    ReleaseControlledPlacementUnit, ReleaseController,
};
pub use runtime::{
    claim_transaction, profile_for_services, release_transaction, validate_registry_contract,
};

const PROFILES_KEY: &str = "placement_profiles";
const TRANSACTIONS_KEY: &str = "placement_transactions";
