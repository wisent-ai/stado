//! Runtime facets and the adapters that implement them.

mod compute;
mod facet;
mod services;
mod storage;

pub use compute::ComputeAdapter;
pub use facet::{RuntimeFacet, SelectionMode};
pub use services::{
    BillingAdapter, DependencyAdapter, ExecutionAdapter, InventoryAdapter, QuotaAdapter,
    RuntimeAdapter,
};
pub use storage::{storage_reach, StorageAdapter, StorageReach};
