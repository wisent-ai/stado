//! Fenced provider-resource leases stored with backend compare-and-swap.
//!
//! Port of `stado/queue/leases/__init__.py`. A lease is a single JSON blob at
//! `provider-leases/{job_id}.json` guarded by an (owner_id, fence_token) pair
//! plus owner/resource TTLs; all mutations go through the backend's
//! conditional-write primitives so a stale owner loses the race.
//!
//! Known Python bug (ported as INTENDED, not as written):
//! `leases/__init__.py` line 143 gates every store operation on
//! `_require_conditional_backend()`, which checks
//! `getattr(storage, "_azure_backend", None)` — an attribute that never
//! exists on Python `JobStorage` (the backend handle is `_blob_backend`), so
//! the check always raises unless the GCS SDK path is present. The intended
//! behavior is "the backend supports conditional writes", which is true for
//! every Rust [`BlobBackend`] (CAS is part of the trait contract), so the
//! gate is a no-op here.
//!
//! The pieces sit beside this entry point: `error` is the lease-layer error,
//! `state` the transition table, `record` the stored lease document with its
//! fence-gated mutations, and `store` the conditional persistence of that
//! document over the configured `JobStorage`.

mod error;
mod record;
mod state;
mod store;

pub use error::LeaseError;
pub use record::ProviderLease;
pub use state::LeaseState;
pub use store::ProviderLeaseStore;
