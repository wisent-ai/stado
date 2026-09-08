//! Every EC2 call [`Ec2Client`] makes: the security-group and subnet
//! lookups RunInstances needs, RunInstances itself, the
//! terminate/stop/start verbs, and the DescribeInstances inventory reads
//! (instance state, the running instance-type census and the live agent
//! refs with their launch age). A trait implementation is one block, so
//! the lifecycle verbs and the inventory reads share this file.

use async_trait::async_trait;
use base64::Engine as _;

use aws_sdk_ec2::types::{
    BlockDeviceMapping, EbsBlockDevice, Filter, IamInstanceProfileSpecification, InstanceType,
    ResourceType, Tag, TagSpecification, VolumeType,
};

use crate::config;
use crate::providers::aws::api::{Ec2Api, RunInstanceArgs};
use crate::providers::aws::diagnostics::ec2_error;
use crate::providers::ProviderError;

use super::Ec2Client;

#[async_trait]
impl Ec2Api for Ec2Client {
    async fn security_group_vpc(&self, group_id: &str) -> Result<String, ProviderError> {
        let out = self
            .client
            .describe_security_groups()
            .group_ids(group_id)
            .send()
            .await
            .map_err(|err| ec2_error("describe_security_groups", &err))?;
        // Python: groups[0]["VpcId"] — an empty list is an IndexError
        // there, an explicit error here.
        let group = out.security_groups().first().ok_or_else(|| {
            ProviderError::Aws(format!(
                "EC2 describe_security_groups failed: no security group {group_id}"
            ))
        })?;
        Ok(group.vpc_id().unwrap_or_default().to_string())
    }

    async fn subnet_in_az(&self, az: &str, vpc_id: &str) -> Result<Option<String>, ProviderError> {
        let out = self
            .client
            .describe_subnets()
            .filters(
                Filter::builder()
                    .name("availability-zone")
                    .values(az)
                    .build(),
            )
            .filters(Filter::builder().name("vpc-id").values(vpc_id).build())
            .send()
            .await
            .map_err(|err| ec2_error("describe_subnets", &err))?;
        Ok(out
            .subnets()
            .first()
            .and_then(|subnet| subnet.subnet_id().map(str::to_string)))
    }

    async fn run_instance(&self, args: &RunInstanceArgs) -> Result<String, ProviderError> {
        // boto3 base64-encodes UserData before sending; the Rust SDK
        // sends the string verbatim ("the base64-encoding might be
        // performed for you" — it isn't here), so encode for wire parity.
        let user_data =
            base64::engine::general_purpose::STANDARD.encode(args.startup_script.as_bytes());
        let out = self
            .client
            .run_instances()
            .image_id(&args.ami_id)
            .instance_type(InstanceType::from(args.machine_type.as_str()))
            .security_group_ids(&args.security_group)
            .subnet_id(&args.subnet_id)
            .iam_instance_profile(
                IamInstanceProfileSpecification::builder()
                    .name(&args.iam_profile)
                    .build(),
            )
            .user_data(user_data)
            .block_device_mappings(
                BlockDeviceMapping::builder()
                    .device_name("/dev/sda1")
                    .ebs(
                        EbsBlockDevice::builder()
                            .volume_size(args.boot_disk_gb as i32)
                            .volume_type(VolumeType::Gp3)
                            .delete_on_termination(true)
                            .build(),
                    )
                    .build(),
            )
            .tag_specifications(
                TagSpecification::builder()
                    .resource_type(ResourceType::Instance)
                    .tags(Tag::builder().key("Name").value(&args.name).build())
                    .build(),
            )
            .min_count(1)
            .max_count(1)
            .send()
            .await
            .map_err(|err| ec2_error("run_instances", &err))?;
        Ok(out
            .instances()
            .first()
            .and_then(|instance| instance.instance_id())
            .unwrap_or_default()
            .to_string())
    }

    async fn terminate_instance(&self, instance_id: &str) -> Result<(), ProviderError> {
        self.client
            .terminate_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .map_err(|err| ec2_error("terminate_instances", &err))?;
        Ok(())
    }

    async fn stop_instance(&self, instance_id: &str) -> Result<(), ProviderError> {
        self.client
            .stop_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .map_err(|error| ec2_error("stop_instances", &error))?;
        Ok(())
    }

    async fn start_instance(&self, instance_id: &str) -> Result<(), ProviderError> {
        self.client
            .start_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .map_err(|error| ec2_error("start_instances", &error))?;
        Ok(())
    }

    async fn instance_state(&self, instance_id: &str) -> Result<Option<String>, ProviderError> {
        let out = self
            .client
            .describe_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .map_err(|err| ec2_error("describe_instances", &err))?;
        // Python: r["Reservations"][0]["Instances"][0]["State"]["Name"] —
        // the IndexError on an empty reservation set surfaces as None
        // here (treated like a missing instance).
        Ok(out
            .reservations()
            .first()
            .and_then(|reservation| reservation.instances().first())
            .and_then(|instance| instance.state())
            .and_then(|state| state.name())
            .map(|name| name.as_str().to_string()))
    }

    async fn running_instance_types(&self) -> Result<Vec<String>, ProviderError> {
        let mut stream = self
            .client
            .describe_instances()
            .filters(
                Filter::builder()
                    .name("tag:Name")
                    .values(format!("{}-*", config::INSTANCE_PREFIX))
                    .build(),
            )
            .filters(
                Filter::builder()
                    .name("instance-state-name")
                    .values("running")
                    .build(),
            )
            .into_paginator()
            .send();
        let mut out = Vec::new();
        while let Some(page) = stream.next().await {
            let page = page.map_err(|err| ec2_error("describe_instances", &err))?;
            for reservation in page.reservations() {
                for instance in reservation.instances() {
                    if let Some(instance_type) = instance.instance_type() {
                        out.push(instance_type.as_str().to_string());
                    }
                }
            }
        }
        Ok(out)
    }

    async fn running_agent_refs_with_age(&self) -> Result<Vec<(String, f64)>, ProviderError> {
        let mut stream = self
            .client
            .describe_instances()
            .filters(
                Filter::builder()
                    .name("tag:Name")
                    .values(format!("{}-agent-*", config::INSTANCE_PREFIX))
                    .build(),
            )
            .filters(
                Filter::builder()
                    .name("instance-state-name")
                    .values("pending")
                    .values("running")
                    .values("stopping")
                    .values("stopped")
                    .build(),
            )
            .into_paginator()
            .send();
        let now = chrono::Utc::now().timestamp();
        let mut out = Vec::new();
        while let Some(page) = stream.next().await {
            let page = page.map_err(|err| ec2_error("describe_instances", &err))?;
            for reservation in page.reservations() {
                for instance in reservation.instances() {
                    let Some(instance_id) = instance.instance_id() else {
                        continue;
                    };
                    let age = instance
                        .launch_time()
                        .map(|created| {
                            now.saturating_sub(created.secs()).max(i64::default()) as f64
                        })
                        .unwrap_or_default();
                    out.push((instance_id.to_string(), age));
                }
            }
        }
        Ok(out)
    }
}
