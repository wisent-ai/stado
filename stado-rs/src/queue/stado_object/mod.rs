//! Shared queue storage through Stado's authenticated object API.
//!
//! Remote local workers cannot use `LocalBackend`: its filesystem is private to
//! one host. This adapter keeps queue paths provider-neutral while the control
//! plane remains the sole owner of the concrete backing store.
//!
//! The seams this module was written along are now its components: the
//! constructor and the pooled HTTPS client behind it ([`client`]), the URI
//! shape every route is addressed through ([`uri`]), the gateway refusals that
//! are a window rather than a verdict ([`refusals`]), and the object reads,
//! writes and metadata that make up the `BlobBackend` surface ([`objects`]).

use std::collections::BTreeMap;

use reqwest::{Client, Url};
use serde::Deserialize;

mod client;
mod objects;
mod refusals;
mod uri;

const VERSION_HEADER: &str = "x-stado-version";

#[derive(Debug)]
pub struct StadoObjectBackend {
    base_url: Url,
    namespace: String,
    token: String,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct ObjectList {
    objects: Vec<ObjectDescriptor>,
}

#[derive(Debug, Deserialize)]
struct ObjectDescriptor {
    key: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}
