//! Bind an absence observation to the account and region of the claimed VM.
use super::Ec2Client;
use crate::providers::aws::api::Ec2Api;
use crate::providers::{InstanceRemovalObservation, ProviderError};

pub(crate) async fn observed_account(
    client: &aws_sdk_sts::Client,
) -> Result<String, ProviderError> {
    // https://docs.aws.amazon.com/STS/latest/APIReference/API_GetCallerIdentity.html
    let response = crate::wait::sdk(client.get_caller_identity().send())
        .await
        .map_err(|error| ProviderError::Aws(format!("STS GetCallerIdentity: {error:?}")))?;
    response
        .account()
        .filter(|account| !account.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::Aws("STS GetCallerIdentity omitted Account".into()))
}

impl Ec2Client {
    pub(super) async fn observe_instance_removal(
        &self,
        account: &str,
        region: &str,
        instance_id: &str,
    ) -> Result<InstanceRemovalObservation, ProviderError> {
        let current_region = self.client.config().region().map(|region| region.as_ref());
        if current_region != Some(region) {
            return Err(ProviderError::Value(format!(
                "EC2 removal scope differs: claimed region {region:?}, client region {current_region:?}"
            )));
        }
        let current_account = observed_account(&self.identity).await?;
        if current_account != account {
            return Err(ProviderError::Value(format!(
                "EC2 removal scope differs: claimed account {account:?}, observed caller account {current_account:?}"
            )));
        }
        let state = self.instance_state(instance_id).await?;
        let removed = state.is_none()
            || state.as_deref() == Some(aws_sdk_ec2::types::InstanceStateName::Terminated.as_str());
        Ok(InstanceRemovalObservation {
            removed,
            state,
            evidence: serde_json::json!({
                "operation": "EC2.DescribeInstances",
                "account_id": current_account, "region": region, "instance_id": instance_id,
            }),
        })
    }
}
