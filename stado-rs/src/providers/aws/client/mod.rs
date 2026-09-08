//! aws-sdk-ec2 transport: the [`Ec2Client`] handle. The `Ec2Api` verbs it
//! implements — the security-group and subnet lookups, RunInstances, the
//! terminate/stop/start calls and the DescribeInstances inventory reads —
//! sit in the `verbs` component; the Skarbiec/IMDSv2 credential chain the
//! handle is built on in `credentials`.

mod credentials;
mod verbs;

use crate::config;
use crate::providers::ProviderError;

pub(crate) use credentials::sdk_config;

/// aws-sdk-ec2 backed `Ec2Api`.
pub struct Ec2Client {
    client: aws_sdk_ec2::Client,
}

impl Ec2Client {
    /// Build the SDK client with the adapter host's IMDSv2 identity.
    pub async fn new() -> Result<Self, ProviderError> {
        let sdk_config = sdk_config(config::aws_region())
            .await
            .map_err(|err| ProviderError::Aws(err.to_string()))?;
        Ok(Ec2Client {
            client: aws_sdk_ec2::Client::new(&sdk_config),
        })
    }
}
