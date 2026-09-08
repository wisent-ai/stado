//! The built-in `activation-dataset` adapter.
//!
//! [`ActivationDatasetAdapter`] itself, its two constructors, the pinned
//! location parse and the shared report shaping live here; the offline
//! inventory check is in `inventory` and the [`ArtifactAdapter`]
//! implementation that fetches the tree around it is in `verify`.

mod helpers;
mod inventory;
mod verify;

use std::sync::Arc;

use serde_json::{Map, Value};

use super::{fetch_hf_tree, ArtifactAdapter, TreeFetcher};
use crate::artifacts_models::{ArtifactManifest, VerificationReport};
use helpers::hf_location_re;

// ---------------------------------------------------------------------------
// ActivationDatasetAdapter
// ---------------------------------------------------------------------------

/// Python `ActivationDatasetAdapter` (`type_name = "activation-dataset"`,
/// `adapter_name = "activation-dataset-v1"`).
pub struct ActivationDatasetAdapter {
    tree_fetcher: TreeFetcher,
}

impl ActivationDatasetAdapter {
    /// Production adapter: lists the pinned revision over the HF HTTP API.
    pub fn new() -> Self {
        Self {
            tree_fetcher: Arc::new(|repo, revision| {
                Box::pin(async move { fetch_hf_tree(&repo, &revision).await })
            }),
        }
    }

    /// Adapter with an injected tree fetcher (tests, offline fixtures).
    pub fn with_fetcher(tree_fetcher: TreeFetcher) -> Self {
        Self { tree_fetcher }
    }

    /// Python `_location`: the (repo, lowercase revision) behind the
    /// primary `hf://datasets/<repo>@<commit>` location, or `None` when the
    /// URI shape or `immutable_revision` cross-check fails.
    fn location(manifest: &ArtifactManifest) -> Option<(String, String)> {
        let primary = manifest
            .locations
            .iter()
            .find(|item| item.role == "primary")?;
        let captures = hf_location_re().captures(&primary.uri)?;
        let (repo, revision) = (&captures[1], &captures[2]);
        // Python compares case-sensitively against the captured revision.
        if !primary.immutable_revision.is_empty() && primary.immutable_revision != revision {
            return None;
        }
        Some((repo.to_string(), revision.to_lowercase()))
    }

    fn report(
        &self,
        passed: bool,
        issues: Vec<String>,
        summary: Map<String, Value>,
    ) -> VerificationReport {
        VerificationReport {
            adapter: self.adapter_name().to_string(),
            passed,
            issues,
            summary,
        }
    }
}

impl Default for ActivationDatasetAdapter {
    fn default() -> Self {
        Self::new()
    }
}
