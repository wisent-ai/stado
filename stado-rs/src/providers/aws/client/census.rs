//! The DescribeInstances census reads behind [`super::Ec2Client`]'s
//! inventory verbs: the instance types running under the fleet's prefix, and
//! every agent instance with its launch age.

use aws_sdk_ec2::types::Filter;

use crate::config;
use crate::providers::aws::diagnostics::ec2_error;
use crate::providers::ProviderError;

use super::Ec2Client;

/// What a paginated read waits on: the AWS API, through its SDK.
const KIND: crate::wait::Kind = crate::wait::Kind::Network;
const AWS: &str = "the AWS API";

impl Ec2Client {
    /// The instance type of every running instance named under the fleet's
    /// prefix.
    pub(super) async fn running_types(&self) -> Result<Vec<String>, ProviderError> {
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
        loop {
            let waiting = crate::wait::begin(KIND, "EC2 DescribeInstances, next page", AWS);
            let Some(page) = stream.next().await else {
                waiting.done();
                break;
            };
            let page = waiting
                .settle(page)
                .map_err(|err| ec2_error("describe_instances", &err))?;
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

    /// Every agent instance under the fleet's prefix, in any live or stopped
    /// state, with the seconds since it launched.
    pub(super) async fn agent_refs_with_age(&self) -> Result<Vec<(String, f64)>, ProviderError> {
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
        loop {
            let waiting = crate::wait::begin(KIND, "EC2 DescribeInstances, next page", AWS);
            let Some(page) = stream.next().await else {
                waiting.done();
                break;
            };
            let page = waiting
                .settle(page)
                .map_err(|err| ec2_error("describe_instances", &err))?;
            for reservation in page.reservations() {
                for instance in reservation.instances() {
                    let Some(instance_id) = instance.instance_id() else {
                        continue;
                    };
                    let age = instance
                        .launch_time()
                        .map(|created| now.saturating_sub(created.secs()).max(0) as f64)
                        .unwrap_or_default();
                    out.push((instance_id.to_string(), age));
                }
            }
        }
        Ok(out)
    }
}
