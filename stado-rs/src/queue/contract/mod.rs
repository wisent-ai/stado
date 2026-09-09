//! Backend-neutral blob contract, split out of `queue/mod.rs`: the
//! [`BlobBackend`] trait, the descriptors and error type its signatures
//! name, the storage-adapter factory, and the Python-compatible JSON
//! serializers.
//!
//! Every name here is re-exported by the parent, so consumers keep naming
//! `crate::queue::<name>` exactly as they did when these bodies sat in
//! `queue/mod.rs`.

mod backend;
mod factory;
mod json;
mod types;

pub use backend::BlobBackend;
pub use types::{BlobInfo, StorageError, VersionedText};

pub(crate) use factory::{construct_backend, BackendLocator};
pub(crate) use json::{json_str, python_json_dumps};
