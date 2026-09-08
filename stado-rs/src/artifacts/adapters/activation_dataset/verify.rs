//! The `ArtifactAdapter` implementation: spec pre-checks, the pinned-tree
//! listing, and the hand-off to the offline inventory check.

use std::collections::HashSet;

use async_trait::async_trait;
use serde_json::{Map, Value};

use super::helpers::str_list;
use super::ActivationDatasetAdapter;
use crate::artifacts::adapters::ArtifactAdapter;
use crate::artifacts_models::{ArtifactManifest, VerificationReport};

#[async_trait]
impl ArtifactAdapter for ActivationDatasetAdapter {
    fn type_name(&self) -> &'static str {
        "activation-dataset"
    }

    fn adapter_name(&self) -> &'static str {
        "activation-dataset-v1"
    }

    async fn verify(&self, manifest: &ArtifactManifest, _full: bool) -> VerificationReport {
        let Some((repo, revision)) = Self::location(manifest) else {
            return self.report(
                false,
                vec!["primary location must be hf://datasets/<repo>@<40-64 hex commit>".into()],
                Map::new(),
            );
        };
        let spec = manifest
            .partitions
            .get("activation_dataset")
            .and_then(Value::as_object);
        let Some(spec) = spec else {
            return self.report(
                false,
                vec!["partitions.activation_dataset specification is required".into()],
                Map::new(),
            );
        };
        // Python validates models/raw/aggregated BEFORE the network call;
        // a cheap pre-check avoids listing the repo for a malformed spec.
        let structural = {
            let mut issues = Vec::new();
            if str_list(spec, "models").is_empty() {
                issues.push("activation_dataset.models must be a non-empty list".to_string());
            }
            if !spec.get("raw").is_some_and(Value::is_object) {
                issues.push("activation_dataset.raw must be an object".to_string());
            }
            if !spec.get("aggregated").is_some_and(Value::is_object) {
                issues.push("activation_dataset.aggregated must be an object".to_string());
            }
            issues
        };
        if !structural.is_empty() {
            return self.report(false, structural, Map::new());
        }
        let files: HashSet<String> = match (self.tree_fetcher)(repo, revision).await {
            Ok(files) => files.into_iter().collect(),
            Err(exc) => {
                return self.report(
                    false,
                    vec![format!(
                        "could not list pinned Hugging Face revision: {exc}"
                    )],
                    Map::new(),
                );
            }
        };
        self.inventory_report(spec, &files)
    }
}
