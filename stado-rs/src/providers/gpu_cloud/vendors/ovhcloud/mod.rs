//! OVHcloud Public Cloud GPU instances.
//!
//! API: <https://docs.ovhcloud.com/en/guides/manage-and-operate/api/first-steps>
//! and the published schema <https://api.ovh.com/1.0/cloud.json> (v1).
//! Every call is signed: `X-Ovh-Signature` is `"$1$" + SHA1_HEX(AS + "+" +
//! CK + "+" + METHOD + "+" + URL + "+" + BODY + "+" + TIMESTAMP)` with the
//! application secret (AS) and consumer key (CK), beside `X-Ovh-Application`,
//! `X-Ovh-Consumer` and `X-Ovh-Timestamp`; the timestamp is OVHcloud's own
//! (`GET /auth/time`), so a skewed local clock cannot void a signature.
//!
//! `POST /cloud/project/{serviceName}/instance` takes `name`, `flavorId`,
//! `imageId`, `region`, `sshKeyId` and plain-text `userData`; `GET` and
//! `DELETE /cloud/project/{serviceName}/instance/{id}` read and release one,
//! and `GET /cloud/project/{serviceName}/instance` lists them all, unpaged,
//! with `created` as the creation time. A flavor is named by id per region,
//! so the adapter reads `GET /cloud/project/{serviceName}/flavor?region=` for
//! the instance type's id; a flavor the region lists as not `available`, or
//! with no `quota` left, is capacity.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret, setting};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::Ovhcloud;
/// The prefix of every signature, the version of the signing scheme.
const SIGNATURE_VERSION: &str = "$1$";
const SIGNATURE_SEPARATOR: &str = "+";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent OVHcloud Public Cloud GPU instances and run an agent on each.",
    config: &[
        // OVHcloud answers on one endpoint per account zone (Europe, Canada,
        // United States); the account's own is declared, never guessed.
        ConfigField::scalar("api", "OVHCLOUD_API", "ovhcloud.api").required(),
        ConfigField::scalar("project", "OVHCLOUD_PROJECT", "ovhcloud.project").required(),
        ConfigField::scalar("region", "OVHCLOUD_REGION", "ovhcloud.region").required(),
        ConfigField::scalar("image-id", "OVHCLOUD_IMAGE_ID", "ovhcloud.image_id").required(),
        ConfigField::scalar("ssh-key-id", "OVHCLOUD_SSH_KEY_ID", "ovhcloud.ssh_key_id"),
    ],
    credential_fields: &["application_key", "application_secret", "consumer_key"],
    offers: &[
        ("l4-90", "nvidia-l4"),
        ("a10-45", "nvidia-a10"),
        ("l40s-90", "nvidia-l40s"),
        ("t1-45", "nvidia-tesla-v100-16gb"),
        ("a100-180", "nvidia-a100-80gb"),
        ("h100-380", "nvidia-h100-80gb"),
    ],
    guest: GuestIdentity::InstanceName,
};

#[derive(Default)]
pub struct Api {
    client: reqwest::Client,
}

impl Api {
    pub fn new() -> Self {
        Self::default()
    }

    /// The account's API endpoint and the project instances live in.
    fn base() -> Result<(String, String), GpuCloudError> {
        let api = required_setting(VENDOR, "api")?;
        let project = required_setting(VENDOR, "project")?;
        Ok((api.trim_end_matches('/').to_string(), project))
    }

    /// OVHcloud's own time, the timestamp a signature carries.
    async fn time(&self, api: &str) -> Result<String, GpuCloudError> {
        let operation = "read OVHcloud's time";
        let answer = http::exchange(
            VENDOR,
            operation,
            self.client.get(format!("{api}/auth/time")),
        )
        .await?;
        match answer {
            Value::Number(seconds) => Ok(seconds.to_string()),
            other => Err(GpuCloudError::response(
                VENDOR,
                operation,
                format!("not a time in seconds: {other}"),
            )),
        }
    }

    /// One signed call; `body` is sent exactly as it is signed.
    async fn call(
        &self,
        operation: &str,
        method: reqwest::Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<Value, GpuCloudError> {
        let (api, _) = Self::base()?;
        let application = secret(VENDOR, "application_key").await?;
        let application_secret = secret(VENDOR, "application_secret").await?;
        let consumer = secret(VENDOR, "consumer_key").await?;
        let text = match body {
            Some(body) => body.to_string(),
            None => String::new(),
        };
        let timestamp = self.time(&api).await?;
        let signed = [
            application_secret.as_str(),
            &consumer,
            method.as_str(),
            url,
            &text,
            &timestamp,
        ]
        .join(SIGNATURE_SEPARATOR);
        let digest =
            ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, signed.as_bytes());
        let signature: String = digest
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut request = self
            .client
            .request(method, url)
            .header("X-Ovh-Application", application)
            .header("X-Ovh-Consumer", consumer)
            .header("X-Ovh-Timestamp", timestamp)
            .header("X-Ovh-Signature", format!("{SIGNATURE_VERSION}{signature}"));
        if body.is_some() {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(text);
        }
        http::exchange(VENDOR, operation, request).await
    }
}

/// An instance's `status`, as `cloud.instance.InstanceStatusEnum` lists it.
fn state(operation: &str, instance: &Value) -> Result<MachineState, GpuCloudError> {
    let status = http::text(VENDOR, operation, instance, "/status")?;
    Ok(match status.as_str() {
        "ACTIVE" => MachineState::Running,
        "BUILD" | "BUILDING" | "REBUILD" | "RESIZE" | "RESIZED" | "VERIFY_RESIZE"
        | "REVERT_RESIZE" | "MIGRATING" | "PASSWORD" | "REBOOT" | "HARD_REBOOT" | "RESUMING"
        | "UNSHELVING" | "UNRESCUING" | "SNAPSHOTTING" => MachineState::Provisioning,
        "SHUTOFF" | "STOPPED" | "SUSPENDED" | "PAUSED" | "SHELVED" | "SHELVED_OFFLOADED"
        | "SHELVING" | "RESCUE" | "RESCUED" | "RESCUING" => MachineState::Stopped,
        "DELETING" => MachineState::Terminating,
        "DELETED" | "SOFT_DELETED" => MachineState::Terminated,
        "ERROR" => MachineState::Failed,
        // Read as running, an instance in a state this Stado does not know
        // would be counted as a working agent.
        other => {
            return Err(GpuCloudError::response(
                VENDOR,
                operation,
                format!("status {other:?} in {instance}"),
            ))
        }
    })
}

/// The instance's flavor name, the instance type Stado offers; the listing
/// carries the flavor whole, a single read may carry only its id.
fn flavor_name(operation: &str, instance: &Value) -> Result<String, GpuCloudError> {
    match http::optional_text(instance, "/flavor/name") {
        Some(name) => Ok(name),
        None => http::text(VENDOR, operation, instance, "/flavorId"),
    }
}

fn machine(operation: &str, instance: &Value) -> Result<Machine, GpuCloudError> {
    Ok(Machine {
        native_id: http::text(VENDOR, operation, instance, "/id")?,
        name: http::optional_text(instance, "/name"),
        instance_type: flavor_name(operation, instance)?,
        state: state(operation, instance)?,
        created_at: http::timestamp(instance, "/created"),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("launch {}", request.instance_type);
        let (api, project) = Self::base()?;
        let region = required_setting(VENDOR, "region")?;
        let image = required_setting(VENDOR, "image-id")?;
        let flavors = self
            .call(
                &format!("read flavors in {region}"),
                reqwest::Method::GET,
                &format!("{api}/cloud/project/{project}/flavor?region={region}"),
                None,
            )
            .await?;
        let listed = flavors.as_array().ok_or_else(|| {
            GpuCloudError::response(VENDOR, &operation, format!("no flavor list in {flavors}"))
        })?;
        let Some(flavor) = listed
            .iter()
            .find(|flavor| flavor["name"].as_str() == Some(request.instance_type))
        else {
            return Err(GpuCloudError::capacity(
                VENDOR,
                request.instance_type,
                format!("region {region} lists no flavor {}", request.instance_type),
            ));
        };
        let available = flavor["available"].as_bool() == Some(true);
        let quota_left = flavor["quota"]
            .as_i64()
            .is_some_and(|quota| quota > Default::default());
        if !available || !quota_left {
            return Err(GpuCloudError::capacity(
                VENDOR,
                request.instance_type,
                format!("region {region} lists the flavor as {flavor}"),
            ));
        }
        let flavor_id = http::text(VENDOR, &operation, flavor, "/id")?;
        let mut body = json!({
            "name": request.name,
            "flavorId": flavor_id,
            "imageId": image,
            "region": region,
            "userData": request.startup_script,
        });
        let key = setting(VENDOR, "ssh-key-id")?;
        if !key.is_empty() {
            body["sshKeyId"] = json!(key);
        }
        let instance = self
            .call(
                &operation,
                reqwest::Method::POST,
                &format!("{api}/cloud/project/{project}/instance"),
                Some(&body),
            )
            .await?;
        machine(&operation, &instance)
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        let (api, project) = Self::base()?;
        self.call(
            &format!("delete instance {native_id}"),
            reqwest::Method::DELETE,
            &format!("{api}/cloud/project/{project}/instance/{native_id}"),
            None,
        )
        .await
        .map(drop)
    }

    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError> {
        let operation = format!("read instance {native_id}");
        let (api, project) = Self::base()?;
        match self
            .call(
                &operation,
                reqwest::Method::GET,
                &format!("{api}/cloud/project/{project}/instance/{native_id}"),
                None,
            )
            .await
        {
            Ok(instance) => machine(&operation, &instance).map(Some),
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list instances";
        let (api, project) = Self::base()?;
        let answer = self
            .call(
                operation,
                reqwest::Method::GET,
                &format!("{api}/cloud/project/{project}/instance"),
                None,
            )
            .await?;
        answer
            .as_array()
            .ok_or_else(|| {
                GpuCloudError::response(VENDOR, operation, format!("no instance list in {answer}"))
            })?
            .iter()
            .map(|instance| machine(operation, instance))
            .collect()
    }
}
