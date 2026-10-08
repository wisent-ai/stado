//! The `Provider` trait implementation for `AwsProvider`: the per-AZ
//! RunInstances attempt that moves on when a zone refuses for capacity,
//! the idempotent terminate, stop/start, and the inventory reads
//! (existence, the accelerator census and the live agent refs with age). A
//! trait implementation is one block, so the lifecycle verbs and the
//! inventory reads share this file.

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::catalog::AWS_INSTANCE_TO_ACCEL;
use crate::providers::aws::api::RunInstanceArgs;
use crate::providers::aws::diagnostics::log;
use crate::providers::{Provider, ProviderError};

use super::{az_order, AwsProvider};

#[async_trait]
impl Provider for AwsProvider {
    #[allow(clippy::too_many_arguments)]
    async fn create_instance(
        &self,
        name: &str,
        machine_type: &str,
        _accel_type: &str,
        boot_disk_gb: i64,
        image: &str,
        _image_project: &str,
        startup_script: &str,
        _preemptible: bool,
    ) -> Result<Option<String>, ProviderError> {
        // AWS Spot instances would require RequestSpotInstances + a
        // different lifecycle than RunInstances. The current
        // implementation always boots on-demand; preemptible=True is
        // accepted for interface compatibility but is not yet wired
        // through.
        let sg = &self.settings.security_group;
        let iam = &self.settings.iam_profile;
        let ami = if self.settings.ami_id.is_empty() {
            image
        } else {
            self.settings.ami_id.as_str()
        };
        let missing: Vec<&str> = [
            ("AWS_SECURITY_GROUP", sg.is_empty()),
            ("AWS_AMI_ID", ami.is_empty()),
            ("AWS_IAM_PROFILE", iam.is_empty()),
        ]
        .into_iter()
        .filter(|(_, absent)| *absent)
        .map(|(name, _)| name)
        .collect();
        if !missing.is_empty() {
            return Err(ProviderError::Value(format!(
                "AWS compute bindings are not declared: {}",
                missing.join(", ")
            )));
        }
        let api = self.api().await?;
        let vpc_id = api.security_group_vpc(sg).await?;

        for az in az_order(&self.settings.region) {
            // One Python loop-body try/except: subnet lookup + run.
            let attempt: Result<Option<String>, ProviderError> = async {
                let Some(subnet_id) = api.subnet_in_az(&az, &vpc_id).await? else {
                    return Ok(None);
                };
                let args = RunInstanceArgs {
                    name: name.to_string(),
                    machine_type: machine_type.to_string(),
                    boot_disk_gb,
                    ami_id: ami.to_string(),
                    startup_script: startup_script.to_string(),
                    security_group: sg.clone(),
                    iam_profile: iam.clone(),
                    subnet_id,
                };
                api.run_instance(&args).await.map(Some)
            }
            .await;
            match attempt {
                Ok(None) => continue,
                Ok(Some(iid)) => {
                    log(&format!("Created {iid} in {az}"));
                    return Ok(Some(iid));
                }
                Err(err) => {
                    if err.to_string().contains("InsufficientInstanceCapacity") {
                        continue;
                    }
                    log(&format!("Failed in {az}: {err}"));
                    continue;
                }
            }
        }
        Ok(None)
    }

    async fn delete_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        self.api().await?.terminate_instance(instance_ref).await
    }

    async fn stop_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        self.api().await?.stop_instance(instance_ref).await
    }

    async fn start_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        self.api().await?.start_instance(instance_ref).await
    }

    async fn instance_exists(&self, instance_ref: &str) -> Result<bool, ProviderError> {
        let state = self.api().await?.instance_state(instance_ref).await?;
        Ok(matches!(state.as_deref(), Some("running" | "pending")))
    }

    async fn instance_removed(
        &self,
        resource: &crate::models::WorkerResource,
    ) -> Result<crate::providers::InstanceRemovalObservation, ProviderError> {
        let crate::models::WorkerResource::Aws {
            account_id,
            region,
            instance_id,
        } = resource
        else {
            return Err(ProviderError::Value(
                "EC2 removal requires an AWS worker identity".into(),
            ));
        };
        self.api()
            .await?
            .instance_removed(account_id, region, instance_id)
            .await
    }

    async fn list_running_instances(&self) -> Result<BTreeMap<String, i64>, ProviderError> {
        let types = self.api().await?.running_instance_types().await?;
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        for instance_type in types {
            if let Some(accel) = AWS_INSTANCE_TO_ACCEL.get(instance_type.as_str()) {
                *counts.entry((*accel).to_string()).or_insert(0) += 1;
            }
        }
        Ok(counts)
    }

    async fn list_running_instance_refs_with_age(
        &self,
    ) -> Result<Vec<(String, f64)>, ProviderError> {
        self.api().await?.running_agent_refs_with_age().await
    }
}
