//! Owner-path writes into a Skarbiec vault.
//!
//! Skarbiec's `PUT /v1/items` requires `id`, `field` and `operation_id`;
//! outside `mode=acquire`, the exact Weles writer must control the item.
//! That field-write route is not a general owner-authorized item write.
//!
//! Owner writes use the `skarbiec` CLI against the vault holding the owner
//! key. Credential creation, rotation and restoration share this path so
//! callers apply the same authorization and storage contract.
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
pub use items::{
    delete_item, item_exists, item_playing_role, list_items, read_document, read_role_string,
    read_string, store_json, write_item, write_role_item,
};
pub use resolution::{candidates_present, vault, VAULT_CANDIDATE_TAILS};
