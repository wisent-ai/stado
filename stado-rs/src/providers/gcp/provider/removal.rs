//! Read physical absence without deleting an agent shared by other jobs.
use super::GcpProvider;
use crate::models::WorkerResource;
use crate::providers::{InstanceRemovalObservation, ProviderError};
use serde_json::{json, Value};

impl GcpProvider {
    pub(super) async fn observe_removal(
        &self,
        resource: &WorkerResource,
    ) -> Result<InstanceRemovalObservation, ProviderError> {
        let WorkerResource::Gcp {
            project_id,
            zone,
            name,
            instance_id,
        } = resource
        else {
            return Err(ProviderError::Value(
                "GCP removal requires a GCP worker identity".into(),
            ));
        };
        if [project_id, zone, name].iter().any(|part| {
            part.is_empty() || part.contains('/') || part.contains('?') || part.contains('#')
        }) {
            return Err(ProviderError::Value(
                "GCP worker identity has an invalid resource path".into(),
            ));
        }
        let state = self.state().await?;
        if state.client.project() != project_id {
            return Err(ProviderError::Value(format!(
                "GCP removal scope differs: claimed project {project_id:?}, client project {:?}",
                state.client.project()
            )));
        }
        let path = format!("/projects/{project_id}/zones/{zone}/instances/{name}");
        let instance = state
            .client
            .get_allow_404(&path, &format!("observe removal of {path}"))
            .await?;
        let observed_id = match &instance {
            Some(instance) => Some(
                instance
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ProviderError::Value(format!("GCP GET {path} omitted instance id"))
                    })?
                    .parse::<u64>()
                    .map_err(|error| {
                        ProviderError::Value(format!(
                            "GCP GET {path} returned invalid instance id: {error}"
                        ))
                    })?,
            ),
            None => None,
        };
        Ok(InstanceRemovalObservation {
            removed: observed_id != Some(*instance_id),
            state: instance
                .as_ref()
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            evidence: json!({
                "operation": "compute.instances.get", "project_id": project_id,
                "zone": zone, "name": name, "instance_id": observed_id,
            }),
        })
    }
}
