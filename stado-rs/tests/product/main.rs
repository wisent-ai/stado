//! Stado's real qualified Rust SDK consumer.
//! Catalog reads and installation refusals inspect actual consumer state.
//! Runtime qualification independently fetches the signed SDK archive and
//! proves that changed executable bytes or source claims cannot be consumed.
//! A missing real SDK release blocks setup; no Python or stub is substituted.

mod catalog;
mod fixture;
mod refusals;
mod runtime;
