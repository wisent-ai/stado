//! AWS provider: EC2 instance lifecycle.
//!
//! Port of `stado/providers/aws.py`. Python uses boto3; this port uses
//! aws-sdk-ec2 + aws-config. `aws_config::defaults` resolves the default
//! credential chain (env -> shared config -> IMDS), the boto3 default
//! chain equivalent; the region comes from the `AWS_REGION` config
//! accessor.
//!
//! Like [`super::gcp::GcpProvider`], the SDK client is resolved lazily on
//! the first API call so `get_provider("aws")` stays a cheap, sync
//! factory (Python's `boto3.client("ec2", ...)` constructor is likewise
//! network-free).
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
    ["a", "c", "d", "b"].iter().map(|suffix| format!("{region}{suffix}")).collect()
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
    /// DescribeInstances state name ("pending"/"running"/...); None when
    /// the reservation set is empty.
    async fn instance_state(&self, instance_id: &str) -> Result<Option<String>, ProviderError>;
    /// Instance types of every running `wisent-*`-tagged instance
    /// (DescribeInstances paginated).
    async fn running_instance_types(&self) -> Result<Vec<String>, ProviderError>;
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
    /// Build the SDK client: default credential chain + AWS_REGION.
    pub async fn new() -> Self {
        let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(config::aws_region().to_string()))
            .load()
            .await;
        Ec2Client { client: aws_sdk_ec2::Client::new(&sdk_config) }
    }
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
            .filters(Filter::builder().name("availability-zone").values(az).build())
            .filters(Filter::builder().name("vpc-id").values(vpc_id).build())
            .send()
            .await
            .map_err(|err| ec2_error("describe_subnets", &err))?;
        Ok(out.subnets().first().and_then(|subnet| subnet.subnet_id().map(str::to_string)))
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
                IamInstanceProfileSpecification::builder().name(&args.iam_profile).build(),
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
            .filters(Filter::builder().name("instance-state-name").values("running").build())
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
        AwsProvider { settings: AwsSettings::from_env(), api: OnceCell::new() }
    }

    /// Bind explicit settings + a fake API (tests).
    #[cfg(test)]
    fn with_api(settings: AwsSettings, api: Arc<dyn Ec2Api>) -> Self {
        let cell = OnceCell::new();
        let _ = cell.set(api);
        AwsProvider { settings, api: cell }
    }

    async fn api(&self) -> &Arc<dyn Ec2Api> {
        self.api
            .get_or_init(|| async { Arc::new(Ec2Client::new().await) as Arc<dyn Ec2Api> })
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
        let ami =
            if self.settings.ami_id.is_empty() { image } else { self.settings.ami_id.as_str() };
        if sg.is_empty() || ami.is_empty() {
            return Err(ProviderError::Value(
                "AWS_SECURITY_GROUP and AWS_AMI_ID are required".to_string(),
            ));
        }
        let api = self.api().await;
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
        match self.api().await.terminate_instance(instance_ref).await {
            Ok(()) => Ok(()),
            // InvalidInstanceID.NotFound is the desired terminal state.
            // Anything else propagates.
            Err(err) if err.to_string().contains("InvalidInstanceID.NotFound") => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn instance_exists(&self, instance_ref: &str) -> Result<bool, ProviderError> {
        match self.api().await.instance_state(instance_ref).await {
            Ok(state) => Ok(matches!(state.as_deref(), Some("running" | "pending"))),
            Err(err) if err.to_string().contains("InvalidInstanceID.NotFound") => Ok(false),
            Err(err) => Err(err),
        }
    }

    async fn list_running_instances(&self) -> Result<BTreeMap<String, i64>, ProviderError> {
        let types = self.api().await.running_instance_types().await?;
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        for instance_type in types {
            if let Some(accel) = AWS_INSTANCE_TO_ACCEL.get(instance_type.as_str()) {
                *counts.entry((*accel).to_string()).or_insert(0) += 1;
            }
        }
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Scripted fake. `run_responses` maps AZ -> queue of outcomes; each
    /// run_instance call pops the front entry (last entry repeats).
    #[derive(Default)]
    struct FakeEc2 {
        subnets: HashMap<String, Option<String>>,
        run_responses: HashMap<String, Vec<Result<String, String>>>,
        run_calls: Mutex<Vec<String>>,
        terminate_responses: Mutex<Vec<Result<(), String>>>,
        terminate_calls: Mutex<Vec<String>>,
        states: HashMap<String, Option<String>>,
        state_errors: HashMap<String, String>,
        running_types: Vec<String>,
    }

    fn aws_err(msg: &str) -> ProviderError {
        ProviderError::Aws(msg.to_string())
    }

    #[async_trait]
    impl Ec2Api for FakeEc2 {
        async fn security_group_vpc(&self, _group_id: &str) -> Result<String, ProviderError> {
            Ok("vpc-123".to_string())
        }
        async fn subnet_in_az(
            &self,
            az: &str,
            _vpc_id: &str,
        ) -> Result<Option<String>, ProviderError> {
            Ok(self.subnets.get(az).cloned().unwrap_or(Some(format!("subnet-{az}"))))
        }
        async fn run_instance(&self, args: &RunInstanceArgs) -> Result<String, ProviderError> {
            let az = args.subnet_id.trim_start_matches("subnet-").to_string();
            self.run_calls.lock().unwrap().push(az.clone());
            let queue = self.run_responses.get(&az);
            match queue.and_then(|q| q.first()) {
                Some(Ok(iid)) => Ok(iid.clone()),
                Some(Err(msg)) => Err(aws_err(msg)),
                None => Ok(format!("i-{az}")),
            }
        }
        async fn terminate_instance(&self, instance_id: &str) -> Result<(), ProviderError> {
            self.terminate_calls.lock().unwrap().push(instance_id.to_string());
            match self.terminate_responses.lock().unwrap().first() {
                Some(Ok(())) | None => Ok(()),
                Some(Err(msg)) => Err(aws_err(msg)),
            }
        }
        async fn instance_state(&self, instance_id: &str) -> Result<Option<String>, ProviderError> {
            if let Some(msg) = self.state_errors.get(instance_id) {
                return Err(aws_err(msg));
            }
            Ok(self.states.get(instance_id).cloned().unwrap_or(None))
        }
        async fn running_instance_types(&self) -> Result<Vec<String>, ProviderError> {
            Ok(self.running_types.clone())
        }
    }

    fn settings() -> AwsSettings {
        AwsSettings {
            region: "us-east-1".to_string(),
            security_group: "sg-123".to_string(),
            iam_profile: "stado-agent".to_string(),
            ami_id: "ami-123".to_string(),
        }
    }

    fn provider(fake: FakeEc2) -> AwsProvider {
        AwsProvider::with_api(settings(), Arc::new(fake))
    }

    #[test]
    fn az_order_matches_python() {
        assert_eq!(
            az_order("us-east-1"),
            vec!["us-east-1a", "us-east-1c", "us-east-1d", "us-east-1b"]
        );
    }

    #[tokio::test]
    async fn create_instance_happy_path_first_az() {
        let p = provider(FakeEc2::default());
        let result = p
            .create_instance("vm1", "g4dn.xlarge", "nvidia-tesla-t4", 200, "ami-ignored", "", "echo hi", false)
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("i-us-east-1a"));
    }

    #[tokio::test]
    async fn create_instance_capacity_fallthrough() {
        let mut fake = FakeEc2::default();
        fake.run_responses.insert(
            "us-east-1a".to_string(),
            vec![Err(
                "EC2 run_instances failed: InsufficientInstanceCapacity: \
                 We currently do not have sufficient capacity"
                    .to_string(),
            )],
        );
        let p = AwsProvider::with_api(settings(), Arc::new(fake));
        let result = p
            .create_instance("vm1", "g4dn.xlarge", "nvidia-tesla-t4", 200, "", "", "echo hi", true)
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("i-us-east-1c"));
    }

    #[tokio::test]
    async fn create_instance_skips_az_without_subnet_and_records_order() {
        let mut raw = FakeEc2::default();
        raw.subnets.insert("us-east-1a".to_string(), None);
        raw.run_responses.insert(
            "us-east-1c".to_string(),
            vec![Err("EC2 run_instances failed: InsufficientInstanceCapacity: x".to_string())],
        );
        let shared = Arc::new(raw);
        let p = AwsProvider::with_api(settings(), shared.clone());
        let result = p
            .create_instance("vm1", "g4dn.xlarge", "", 200, "", "", "echo hi", false)
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("i-us-east-1d"));
        let calls = shared.run_calls.lock().unwrap().clone();
        // a: no subnet (skipped, no run call); c: capacity error; d: ok.
        assert_eq!(calls, vec!["us-east-1c", "us-east-1d"]);
    }

    #[tokio::test]
    async fn create_instance_all_azs_fail_returns_none() {
        let mut raw = FakeEc2::default();
        for az in ["us-east-1a", "us-east-1c", "us-east-1d", "us-east-1b"] {
            raw.run_responses.insert(
                az.to_string(),
                vec![Err("EC2 run_instances failed: UnauthorizedOperation: nope".to_string())],
            );
        }
        let p = provider(raw);
        let result = p
            .create_instance("vm1", "g4dn.xlarge", "", 200, "", "", "echo hi", false)
            .await
            .unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn create_instance_requires_sg_and_ami() {
        let mut s = settings();
        s.security_group = String::new();
        let p = AwsProvider::with_api(s, Arc::new(FakeEc2::default()));
        let err = p
            .create_instance("vm1", "g4dn.xlarge", "", 200, "ami-x", "", "echo hi", false)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "AWS_SECURITY_GROUP and AWS_AMI_ID are required");

        // Empty AWS_AMI_ID falls back to the per-job image argument.
        let mut s2 = settings();
        s2.ami_id = String::new();
        let shared = Arc::new(FakeEc2::default());
        let p2 = AwsProvider::with_api(s2, shared.clone());
        assert!(p2
            .create_instance("vm1", "g4dn.xlarge", "", 200, "ami-job", "", "echo hi", false)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn delete_instance_notfound_is_success() {
        let shared = Arc::new(FakeEc2::default());
        shared
            .terminate_responses
            .lock()
            .unwrap()
            .push(Err("EC2 terminate_instances failed: InvalidInstanceID.NotFound: \
                 The instance ID 'i-gone' does not exist"
                .to_string()));
        let p = AwsProvider::with_api(settings(), shared.clone());
        p.delete_instance("i-gone").await.unwrap();
        assert_eq!(shared.terminate_calls.lock().unwrap().as_slice(), &["i-gone"]);

        // Other errors propagate.
        let other = Arc::new(FakeEc2::default());
        other
            .terminate_responses
            .lock()
            .unwrap()
            .push(Err("EC2 terminate_instances failed: UnauthorizedOperation: no".to_string()));
        let p2 = AwsProvider::with_api(settings(), other);
        assert!(p2.delete_instance("i-x").await.is_err());
    }

    #[tokio::test]
    async fn instance_exists_state_mapping() {
        let mut raw = FakeEc2::default();
        raw.states.insert("i-run".to_string(), Some("running".to_string()));
        raw.states.insert("i-pend".to_string(), Some("pending".to_string()));
        raw.states.insert("i-term".to_string(), Some("terminated".to_string()));
        raw.state_errors.insert(
            "i-gone".to_string(),
            "EC2 describe_instances failed: InvalidInstanceID.NotFound: x".to_string(),
        );
        let p = provider(raw);
        assert!(p.instance_exists("i-run").await.unwrap());
        assert!(p.instance_exists("i-pend").await.unwrap());
        assert!(!p.instance_exists("i-term").await.unwrap());
        assert!(!p.instance_exists("i-gone").await.unwrap());
    }

    #[tokio::test]
    async fn list_running_instances_counts_known_types() {
        let raw = FakeEc2 {
            running_types: vec![
                "g4dn.xlarge".to_string(),
                "g4dn.xlarge".to_string(),
                "p5.4xlarge".to_string(),
                "t3.micro".to_string(), // not in the accel map: skipped
            ],
            ..FakeEc2::default()
        };
        let p = provider(raw);
        let counts = p.list_running_instances().await.unwrap();
        assert_eq!(
            counts,
            BTreeMap::from([
                ("nvidia-tesla-t4".to_string(), 2),
                ("nvidia-h100-80gb".to_string(), 1),
            ])
        );
    }
}
