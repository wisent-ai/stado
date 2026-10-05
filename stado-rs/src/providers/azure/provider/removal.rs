//! A resource name can be reused; only the recorded VM generation counts.
use super::super::arm::COMPUTE_API_VERSION;
use super::super::builders::power_state;
use super::AzureProvider;
use crate::models::WorkerResource;
use crate::providers::{InstanceRemovalObservation, ProviderError};
use serde_json::{json, Value};

impl AzureProvider {
    pub(super) async fn observe_removal(
        &self,
        resource: &WorkerResource,
    ) -> Result<InstanceRemovalObservation, ProviderError> {
        let WorkerResource::Azure {
            subscription_id,
            resource_id,
            name,
            vm_id,
            ..
        } = resource
        else {
            return Err(ProviderError::Value(
                "Azure removal requires an Azure worker identity".into(),
            ));
        };
        let mut parts = resource_id.split('/');
        let valid_path = parts.next() == Some("")
            && parts
                .next()
                .is_some_and(|part| part.eq_ignore_ascii_case("subscriptions"))
            && parts
                .next()
                .is_some_and(|part| !part.is_empty() && part.eq_ignore_ascii_case(subscription_id))
            && parts
                .next()
                .is_some_and(|part| part.eq_ignore_ascii_case("resourceGroups"))
            && parts.next().is_some_and(|part| !part.is_empty())
            && parts
                .next()
                .is_some_and(|part| part.eq_ignore_ascii_case("providers"))
            && parts
                .next()
                .is_some_and(|part| part.eq_ignore_ascii_case("Microsoft.Compute"))
            && parts
                .next()
                .is_some_and(|part| part.eq_ignore_ascii_case("virtualMachines"))
            && parts
                .next()
                .is_some_and(|part| !part.is_empty() && part.eq_ignore_ascii_case(name))
            && parts.next().is_none();
        if !valid_path || resource_id.contains('?') || resource_id.contains('#') || vm_id.is_empty()
        {
            return Err(ProviderError::Value(
                "Azure worker identity has an invalid VM resource path or generation".into(),
            ));
        }
        let state = self.state().await?;
        if !state
            .client
            .subscription()
            .eq_ignore_ascii_case(subscription_id)
        {
            return Err(ProviderError::Value(format!(
                "Azure removal scope differs: claimed subscription {subscription_id:?}, client subscription {:?}",
                state.client.subscription()
            )));
        }
        let path = format!("{resource_id}?$expand=instanceView&api-version={COMPUTE_API_VERSION}");
        let vm = state
            .client
            .get_allow_404(&path, &format!("observe removal of {resource_id}"))
            .await?;
        let observed_id = match &vm {
            Some(vm) => Some(
                vm.pointer("/properties/vmId")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        ProviderError::Value(format!(
                            "Azure GET {resource_id} omitted properties.vmId"
                        ))
                    })?,
            ),
            None => None,
        };
        Ok(InstanceRemovalObservation {
            removed: observed_id.is_none_or(|observed| !observed.eq_ignore_ascii_case(vm_id)),
            state: vm.as_ref().and_then(power_state),
            evidence: json!({
                "operation": "Microsoft.Compute.virtualMachines.get",
                "subscription_id": subscription_id, "resource_id": resource_id, "vm_id": observed_id,
            }),
        })
    }
}
