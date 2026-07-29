//! Autonomous multi-cloud resource and FinOps control plane.
//!
//! The module deliberately reuses the canonical queue storage, provider
//! adapters, resource planner, and operation journal. It is not a parallel
//! control plane: inventory, economic placement, reconciliation, and savings
//! measurement are additional coordinator stages over the same state.

pub mod advisor;
pub mod cost;
pub mod inventory;
pub mod lifecycle;
pub mod model;
pub mod optimizer;
pub mod policy;
pub mod reconciler;
pub mod storage;

pub use model::{DecisionRecord, InventorySnapshot, ResourceRecord, SavingsRecord};
pub use policy::{AutonomyMode, AutonomyPolicy};
