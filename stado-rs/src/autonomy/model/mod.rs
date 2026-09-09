//! Provider-neutral contracts for autonomous resource and cost management.
//!
//! The components are the record families this file already separated:
//! `resources` holds the schema version, the ownership and source-state
//! vocabulary, the resource record with its canonical identity, and the
//! inventory source, snapshot and dependency graph built from those records;
//! `decisions` holds the decision one pass emits and the savings, measurement
//! and adoption records that follow from it. Every item stays published here,
//! so `crate::autonomy::model::<item>` resolves exactly as before.

mod decisions;
mod resources;

pub use decisions::{
    AdoptionRecord, DecisionKind, DecisionRecord, SavingsMeasurement, SavingsRecord,
};
pub use resources::{
    canonical_resource_id, InventorySnapshot, InventorySource, Ownership, ResourceGraph,
    ResourceRecord, SourceState, SCHEMA_VERSION,
};
