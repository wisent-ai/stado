//! Owner-path writes into a Skarbiec vault.
//!
//! Skarbiec's `PUT /v1/items` is not a general item write and has not been one
//! since the vault contracts were rebuilt (Skarbiec 9aa7dd4, 2026-08-04). The
//! route now requires `id`, `field` and `operation_id`, and outside
//! `mode=acquire` it refuses anything that is not controlled by the exact Weles
//! writer presenting the grant. Stado's client still sent the whole item, so the
//! broker answered every write — `stado credentials put`, `stado fleet key
//! generate`, `key add`, `key rotate`, the Azure operator credential — with a
//! bare `400 {"error":"field required"}`. The fleet could read its credentials
//! and could not mint one, which is why a new host could not be enrolled at all.
//!
//! An item the operator owns is written the way its owner writes it: through the
//! `skarbiec` CLI against the vault file, which holds the owner key. That is the
//! same call `stado credentials harvest --restore` already made for a Skarbiec
//! selector; it lives here now so every write in the process shares it instead
//! of one path knowing the contract and the rest guessing.
//!
//! Field placement belongs to Skarbiec's schema, not to callers: a `ssh-key`
//! payload normalizes to kind `key-pair` with `private_key`/`public_key` as
//! fields and `fingerprint`/`key_type` as context. Sending the flat object is
//! correct; assuming where each key lands on the way out is not.

mod discovery;
mod host_authority;
mod items;
mod resolution;

pub use discovery::binary;
pub use host_authority::authority;
pub use items::{delete_item, item_exists, read_string, store_json, write_item};
pub use resolution::{candidates_present, vault, VAULT_CANDIDATE_TAILS};
