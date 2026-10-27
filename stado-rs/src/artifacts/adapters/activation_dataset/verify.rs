//! The `ArtifactAdapter` implementation: spec pre-checks, the pinned-tree
//! listing, and the hand-off to the offline inventory check.

use std::collections::HashSet;

use async_trait::async_trait;
use serde_json::{Map, Value};

use super::helpers::DatasetSpec;
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
        // The specification is read before the network call: a malformed one
        // is named without listing the repository.
        let spec = match DatasetSpec::read(spec) {
            Ok(spec) => spec,
            Err(issues) => return self.report(false, issues, Map::new()),
        };
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
        self.inventory_report(&spec, &files)
    }
}
