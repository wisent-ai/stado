//! The provider-neutral product objects: the object-API client, the origins
//! it reaches, the store-and-fetch helpers the rest of the crate calls, and
//! the commands over them.

// ---- provider-neutral product objects ----

pub(in crate::cli::storage) mod api;
pub(in crate::cli::storage) mod endpoint;
pub(in crate::cli::storage) mod store;
pub(in crate::cli::storage) mod verbs;
