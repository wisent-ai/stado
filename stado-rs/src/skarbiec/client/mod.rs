//! The Skarbiec HTTP client: constructor, request plumbing, and item CRUD.
//! Verifier-grant constructors live in `verifiers/`; this is the plain
//! client every read path ultimately talks through.
//!
//! The bodies live beside this entry point: `ctor` builds the client and its
//! HTTP handle, `transport` mints the grant and carries one request, `reads`
//! answers item and field reads, and `writes` performs the owner acts.

mod ctor;
mod reads;
mod transport;
mod writes;

use super::GrantMode;

pub struct Client {
    http: reqwest::Client,
    base_url: String,
    consumer: String,
    token_file: String,
    route_store: bool,
    grant_mode: GrantMode,
}
