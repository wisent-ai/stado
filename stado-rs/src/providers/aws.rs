//! AWS provider: EC2 instance lifecycle.
//!
//! Port of `stado/providers/aws.py`. Python uses boto3; this port uses
//! aws-sdk-ec2 + aws-config. The coordinator reads `stado-aws` through its
//! scoped Skarbiec grant; adapter hosts without a Skarbiec grant use their
//! EC2 IMDSv2 workload identity. Environment credential chains are disabled.
//!
//! Like [`super::gcp::GcpProvider`], the SDK client is resolved lazily on
//! the first API call so `get_provider("aws")` stays a cheap, sync
//! factory.
//!
//! Deviation: the instance_ref is the raw EC2 instance id (Python
//! returns `iid` and `delete_instance`/`instance_exists` pass it back to
//! EC2 verbatim), not the `"name@zone"` shape gcp/azure use.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use tokio::sync::OnceCell;

use aws_sdk_ec2::types::{
    BlockDeviceMapping, EbsBlockDevice, Filter, IamInstanceProfileSpecification, InstanceType,
    ResourceType, Tag, TagSpecification, VolumeType,
};

use crate::catalog::AWS_INSTANCE_TO_ACCEL;
use crate::config;

use super::{Provider, ProviderError};

/// Python `_log`.
fn log(msg: &str) {
    eprintln!("[aws] {msg}");
}

/// Python `AZ_ORDER` — `[f"{REGION}{suffix}" for suffix in a,c,d,b]`.
pub fn az_order(region: &str) -> Vec<String> {
    ["a", "c", "d", "b"]
        .iter()
        .map(|suffix| format!("{region}{suffix}"))
        .collect()
}

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

/// Lift an [`aws_sdk_ec2::error::SdkError`] into [`ProviderError::Aws`],
/// embedding the service error code so Python's substring classification
/// keeps working on the message.
fn ec2_error<E>(desc: &str, err: &aws_sdk_ec2::error::SdkError<E>) -> ProviderError
where
    E: aws_sdk_ec2::error::ProvideErrorMetadata + std::fmt::Debug,
{
    if let Some(service) = err.as_service_error() {
        let code = service.code().unwrap_or("");
        let message = service.message().unwrap_or("");
        return ProviderError::Aws(format!("EC2 {desc} failed: {code}: {message}"));
    }
    ProviderError::Aws(format!("EC2 {desc} failed: {err}"))
}

/// aws-sdk-ec2 backed [`Ec2Api`].
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

/// One `stado-aws` field, tried under each accepted name.
///
/// The broker requires a named field on `/v1/items/read`; a whole-item read
/// answers `HTTP 400 {"error":"field required"}`. That refusal used to reach
/// the operator as an AWS-credential failure while the item was readable.
///
/// A refusal on one candidate name does not end the search: the grant may
/// name `aws_access_key_id` where the first attempt asked for
/// `access_key_id`. The last refusal is returned only when no name resolved,
/// so an unauthorized grant still surfaces its own error rather than a
/// misleading "missing value".
async fn stado_aws_field(names: &[&str]) -> Result<Option<String>, crate::skarbiec::SkarbiecError> {
    let mut refusal = None;
    for name in names {
        match crate::skarbiec::read_string("stado-aws", name).await {
            Ok(Some(value)) if !value.trim().is_empty() => return Ok(Some(value)),
            Ok(_) => {}
            Err(error) => refusal = Some(error),
        }
    }
    match refusal {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

/// AWS SDK configuration from either the coordinator's scoped `stado-aws`
/// Skarbiec item or, on adapter hosts without a grant, the host's IMDSv2
/// workload identity. Process-environment and shared-profile credential
/// chains are deliberately bypassed.
pub(crate) async fn sdk_config(
    region: &str,
) -> Result<aws_config::SdkConfig, crate::skarbiec::SkarbiecError> {
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
    if !region.is_empty() {
        loader = loader.region(aws_config::Region::new(region.to_string()));
    }
    if !crate::config::skarbiec_consumer().trim().is_empty()
        && !crate::config::skarbiec_token_file().trim().is_empty()
    {
        let access_key = stado_aws_field(&["access_key_id", "aws_access_key_id"])
            .await?
            .ok_or_else(|| {
                crate::skarbiec::SkarbiecError::MissingValue("stado-aws.access_key_id".to_string())
            })?;
        let secret_key = stado_aws_field(&["secret_access_key", "aws_secret_access_key"])
            .await?
            .ok_or_else(|| {
                crate::skarbiec::SkarbiecError::MissingValue(
                    "stado-aws.secret_access_key".to_string(),
                )
            })?;
        let session_token = stado_aws_field(&["session_token", "aws_session_token"]).await?;
        let credentials = aws_sdk_ec2::config::Credentials::new(
            access_key,
            secret_key,
            session_token,
            None,
            "Skarbiec",
        );
        loader = loader.credentials_provider(credentials);
    } else {
        let imds = aws_config::imds::credentials::ImdsCredentialsProvider::builder().build();
        aws_sdk_ec2::config::ProvideCredentials::provide_credentials(&imds)
            .await
            .map_err(|error| {
                crate::skarbiec::SkarbiecError::Deployment(format!(
                    "AWS adapter IMDSv2 identity is unavailable: {error}"
                ))
            })?;
        loader = loader.credentials_provider(imds);
    }
    Ok(loader.load().await)
}

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

/// Env-resolved settings for the provider (Python reads them from
/// os.environ in create_instance; resolved once at construction here).
#[derive(Clone)]
pub struct AwsSettings {
    pub region: String,
    pub security_group: String,
    pub iam_profile: String,
    pub ami_id: String,
}

impl AwsSettings {
    pub fn from_env() -> Self {
        AwsSettings {
            region: config::aws_region().to_string(),
            security_group: config::aws_security_group().to_string(),
            iam_profile: config::aws_iam_profile().to_string(),
            ami_id: config::aws_ami_id().to_string(),
        }
    }
}

/// Python `AWSProvider`.
pub struct AwsProvider {
    settings: AwsSettings,
    api: OnceCell<Arc<dyn Ec2Api>>,
}

impl AwsProvider {
    /// Python `AWSProvider()` — the SDK client itself resolves lazily on
    /// the first API call (see the module docs).
    pub fn from_env() -> Self {
        AwsProvider {
            settings: AwsSettings::from_env(),
            api: OnceCell::new(),
        }
    }

    /// Bind explicit settings + a fake API (tests).

    async fn api(&self) -> Result<&Arc<dyn Ec2Api>, ProviderError> {
        self.api
            .get_or_try_init(|| async { Ok(Arc::new(Ec2Client::new().await?) as Arc<dyn Ec2Api>) })
            .await
    }
}

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
        if sg.is_empty() || ami.is_empty() {
            return Err(ProviderError::Value(
                "AWS_SECURITY_GROUP and AWS_AMI_ID are required".to_string(),
            ));
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
        match self.api().await?.terminate_instance(instance_ref).await {
            Ok(()) => Ok(()),
            // InvalidInstanceID.NotFound is the desired terminal state.
            // Anything else propagates.
            Err(err) if err.to_string().contains("InvalidInstanceID.NotFound") => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn stop_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        self.api().await?.stop_instance(instance_ref).await
    }

    async fn start_instance(&self, instance_ref: &str) -> Result<(), ProviderError> {
        self.api().await?.start_instance(instance_ref).await
    }

    async fn instance_exists(&self, instance_ref: &str) -> Result<bool, ProviderError> {
        match self.api().await?.instance_state(instance_ref).await {
            Ok(state) => Ok(matches!(state.as_deref(), Some("running" | "pending"))),
            Err(err) if err.to_string().contains("InvalidInstanceID.NotFound") => Ok(false),
            Err(err) => Err(err),
        }
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

