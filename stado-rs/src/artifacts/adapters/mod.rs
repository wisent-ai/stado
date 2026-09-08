//! Artifact-type verification adapters.
//!
//! Port of `stado/artifacts/adapters/{base,__init__,activations}.py`.
//!
//! DEVIATION: Python keeps a mutable `_ADAPTERS` dict populated by
//! `register_adapter` (with an entry-points-style plugin design); Rust has
//! no entry_points, so the registry is static — [`get_adapter`] is a single
//! match/factory. Adding an adapter = add one arm returning
//! `Box<dyn ArtifactAdapter>`. The trait-object design is kept so the
//! registry and CLI stay adapter-agnostic.
//!
//! The built-in `activation-dataset` adapter verifies the HF dataset tree
//! through the Hugging Face HTTP API (`fetch_hf_tree`, reqwest +
//! `HF_TOKEN`); in Python that listing goes through `huggingface_hub`'s
//! underlying HTTP endpoint via urllib. The inventory checks themselves
//! are pure ([`ActivationDatasetAdapter::inventory_report`]) and tested
//! offline; the tree fetcher is injectable for tests.

mod activation_dataset;
mod activation_manifest;
mod contract;
mod hf_tree;

pub use activation_dataset::ActivationDatasetAdapter;
pub use activation_manifest::build_activation_manifest;
pub use contract::{get_adapter, ArtifactAdapter};
pub use hf_tree::{fetch_hf_tree, TreeFetchError, TreeFetcher};
