//! Direct provider-API inventory normalized into the autonomy resource model.
//!
//! The components are the seams this file already carried: [`sources`] holds
//! one reader per place a resource lives — the local capacity publications
//! and the GCP, AWS and Azure provider APIs — [`values`] holds the payload
//! reads, the reference walk and the digests every reader shares, and
//! [`snapshot`] is the report those readers feed: it fans them out, folds the
//! adoptions in, resolves the dependency edges and seals the result. Every
//! name a caller outside this module uses is re-exported here, so
//! `crate::autonomy::inventory::<item>` resolves exactly as before.

mod snapshot;
mod sources;
mod values;

// `super::storage` for the moved lines that name `super::storage::<item>`
// verbatim in `snapshot`.
use crate::autonomy::storage;

pub use snapshot::{collect, collect_and_publish, reseal};
