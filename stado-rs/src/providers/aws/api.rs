//! The EC2 contract the provider codes against: the [`Ec2Api`] trait and
//! the [`RunInstanceArgs`] bundle one RunInstances attempt takes. The
//! aws-sdk-ec2 implementation lives in `client`, its only caller in
//! `provider`.

use async_trait::async_trait;

use crate::providers::ProviderError;

/// Inputs for one RunInstances attempt (the Python `run_instances(...)`
/// kwargs, minus the per-AZ subnet which is resolved first).
pub struct RunInstanceArgs {
    pub name: String,
    pub machine_type: String,
    pub boot_disk_gb: i64,
    pub ami_id: String,
    pub startup_script: String,
    pub security_group: String,
    pub iam_profile: String,
    pub subnet_id: String,
}

/// The EC2 operations the provider uses, behind a trait so tests inject
/// fakes (no live AWS calls). Error messages carry the EC2 error code
/// (e.g. `InsufficientInstanceCapacity`, `InvalidInstanceID.NotFound`) so
/// the Python substring classification works on `error.to_string()`.
#[async_trait]
pub trait Ec2Api: Send + Sync {
    /// VpcId of the given security group (DescribeSecurityGroups).
    async fn security_group_vpc(&self, group_id: &str) -> Result<String, ProviderError>;
    /// First subnet in (az, vpc); None when the AZ has none.
    async fn subnet_in_az(&self, az: &str, vpc_id: &str) -> Result<Option<String>, ProviderError>;
    /// RunInstances; returns the instance id.
    async fn run_instance(&self, args: &RunInstanceArgs) -> Result<String, ProviderError>;
    /// TerminateInstances.
    async fn terminate_instance(&self, instance_id: &str) -> Result<(), ProviderError>;
    async fn stop_instance(&self, _instance_id: &str) -> Result<(), ProviderError> {
        Err(ProviderError::NotImplemented(
            "EC2 adapter does not support stop_instances".to_string(),
        ))
    }
    async fn start_instance(&self, _instance_id: &str) -> Result<(), ProviderError> {
        Err(ProviderError::NotImplemented(
            "EC2 adapter does not support start_instances".to_string(),
        ))
    }
    /// DescribeInstances state name ("pending"/"running"/...); None when
    /// the reservation set is empty.
    async fn instance_state(&self, instance_id: &str) -> Result<Option<String>, ProviderError>;
    /// Instance types of every running `wisent-*`-tagged instance
    /// (DescribeInstances paginated).
    async fn running_instance_types(&self) -> Result<Vec<String>, ProviderError>;
    /// Live Stado agent instance ids and their launch age.
    async fn running_agent_refs_with_age(&self) -> Result<Vec<(String, f64)>, ProviderError> {
        Ok(Vec::new())
    }
}
