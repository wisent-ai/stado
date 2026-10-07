//! Nebius AI Cloud Compute instances.
//!
//! API: <https://docs.nebius.com/rest-api> (REST gateway over the
//! `nebius.compute.v1.InstanceService` of <https://github.com/nebius/api>).
//! A service account authenticates by signing an RS256 JWT (`kid` = its
//! authorized key id, `iss` = `sub` = its id) and exchanging it at
//! `auth.eu.nebius.com/oauth2/token/exchange` for an access token, which is
//! cached until the `expires_in` the exchange reports. `POST
//! /compute/v1/instances` takes `metadata` (`parent_id`, `name`) and `spec`
//! (`resources.platform` and `preset`, `boot_disk` as a managed disk from an
//! image family, `network_interfaces`, `cloud_init_user_data`, `hostname`);
//! `DELETE /compute/v1/instances/{id}` deletes it with its managed disk;
//! `GET /compute/v1/instances/{id}` and the token-paged `GET
//! /compute/v1/instances?parent_id=` read them. The instance type is
//! `<platform>/<preset>`.

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::vendors::signing;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::Nebius;
const API: &str = "https://api.nebius.cloud/compute/v1/instances";
const TOKEN_EXCHANGE: &str = "https://auth.eu.nebius.com/oauth2/token/exchange";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent Nebius AI Cloud GPU instances and run an agent on each.",
    config: &[
        ConfigField::scalar("project-id", "NEBIUS_PROJECT_ID", "nebius.project_id").required(),
        ConfigField::scalar("subnet-id", "NEBIUS_SUBNET_ID", "nebius.subnet_id").required(),
        ConfigField::scalar("image-family", "NEBIUS_IMAGE_FAMILY", "nebius.image_family").required(),
        ConfigField::scalar(
            "jwt-lifetime-seconds",
            "NEBIUS_JWT_LIFETIME_SECONDS",
            "nebius.jwt_lifetime_seconds",
        )
        .required(),
    ],
    credential_fields: &["service_account_id", "key_id", "private_key"],
    offers: &[
        ("gpu-l40s-a/1gpu-8vcpu-32gb", "nvidia-l40s"),
        ("gpu-h100-sxm/1gpu-16vcpu-200gb", "nvidia-h100-80gb"),
        ("gpu-h200-sxm/1gpu-16vcpu-200gb", "nvidia-h200-141gb"),
    ],
    guest: GuestIdentity::InstanceName,
};

#[derive(Default)]
pub struct Api {
    client: reqwest::Client,
    token: Mutex<Option<(String, chrono::DateTime<chrono::Utc>)>>,
}

impl Api {
    pub fn new() -> Self {
        Self::default()
    }

    /// A service-account JWT, signed now.
    async fn assertion(&self) -> Result<String, GpuCloudError> {
        let account = secret(VENDOR, "service_account_id").await?;
        let key_id = secret(VENDOR, "key_id").await?;
        let private_key = secret(VENDOR, "private_key").await?;
        let raw = required_setting(VENDOR, "jwt-lifetime-seconds")?;
        let lifetime: i64 = raw.parse().map_err(|_| {
            GpuCloudError::Configuration(format!(
                "Nebius AI Cloud: nebius.jwt_lifetime_seconds must be whole seconds, not {raw:?}"
            ))
        })?;
        let now = chrono::Utc::now().timestamp();
        let header = json!({"alg": "RS256", "typ": "JWT", "kid": key_id});
        let claims = json!({"iss": account, "sub": account, "iat": now, "exp": now + lifetime});
        let signing_input = format!(
            "{}.{}",
            signing::base64_url(header.to_string().as_bytes()),
            signing::base64_url(claims.to_string().as_bytes())
        );
        let signature = signing::rsa_sha256(VENDOR, &private_key, signing_input.as_bytes())?;
        Ok(format!("{signing_input}.{}", signing::base64_url(&signature)))
    }

    /// The cached access token, exchanged again once it has expired.
    async fn access_token(&self) -> Result<String, GpuCloudError> {
        let mut cached = self.token.lock().await;
        if let Some((token, expires)) = cached.as_ref() {
            if *expires > chrono::Utc::now() {
                return Ok(token.clone());
            }
        }
        let operation = "exchange the service-account JWT for an access token";
        let form = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange".to_string()),
            ("requested_token_type", "urn:ietf:params:oauth:token-type:access_token".to_string()),
            ("subject_token", self.assertion().await?),
            ("subject_token_type", "urn:ietf:params:oauth:token-type:jwt".to_string()),
        ];
        let answer =
            http::exchange(VENDOR, operation, self.client.post(TOKEN_EXCHANGE).form(&form)).await?;
        let token = http::text(VENDOR, operation, &answer, "/access_token")?;
        let lifetime = answer
            .get("expires_in")
            .and_then(Value::as_i64)
            .ok_or_else(|| GpuCloudError::response(VENDOR, operation, "no expires_in in the answer"))?;
        let expires = chrono::Utc::now() + chrono::TimeDelta::seconds(lifetime);
        *cached = Some((token.clone(), expires));
        Ok(token)
    }

    async fn call(
        &self,
        operation: &str,
        method: reqwest::Method,
        url: String,
        body: Option<&Value>,
    ) -> Result<Value, GpuCloudError> {
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(self.access_token().await?);
        if let Some(body) = body {
            request = request.json(body);
        }
        http::exchange(VENDOR, operation, request).await
    }
}

fn machine(operation: &str, instance: &Value) -> Result<Machine, GpuCloudError> {
    let state = match http::optional_text(instance, "/status/state").as_deref() {
        Some("RUNNING") => MachineState::Running,
        Some("STOPPING") => MachineState::Stopping,
        Some("STOPPED") => MachineState::Stopped,
        Some("DELETING") => MachineState::Terminating,
        Some("ERROR") => MachineState::Failed,
        _ => MachineState::Provisioning,
    };
    Ok(Machine {
        native_id: http::text(VENDOR, operation, instance, "/metadata/id")?,
        name: http::optional_text(instance, "/metadata/name").unwrap_or_default(),
        instance_type: format!(
            "{}/{}",
            http::text(VENDOR, operation, instance, "/spec/resources/platform")?,
            http::text(VENDOR, operation, instance, "/spec/resources/preset")?
        ),
        state,
        created_at: http::timestamp(instance, "/metadata/created_at"),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("create instance {}", request.instance_type);
        let Some((platform, preset)) = request.instance_type.split_once('/') else {
            return Err(GpuCloudError::Configuration(format!(
                "Nebius AI Cloud: instance type {:?} is not <platform>/<preset>",
                request.instance_type
            )));
        };
        let body = json!({
            "metadata": {
                "parent_id": required_setting(VENDOR, "project-id")?,
                "name": request.name,
            },
            "spec": {
                "resources": {"platform": platform, "preset": preset},
                "boot_disk": {
                    "attach_mode": "READ_WRITE",
                    "managed_disk": {
                        "name": format!("{}-boot", request.name),
                        "spec": {
                            "size_gibibytes": request.boot_disk_gb,
                            "type": "NETWORK_SSD",
                            "source_image_family": {
                                "image_family": required_setting(VENDOR, "image-family")?,
                            },
                        },
                    },
                },
                "network_interfaces": [{
                    "name": "primary",
                    "subnet_id": required_setting(VENDOR, "subnet-id")?,
                    "ip_address": {},
                    "public_ip_address": {},
                }],
                "cloud_init_user_data": request.startup_script,
                "hostname": request.name,
            },
        });
        let operation_answer = self
            .call(&operation, reqwest::Method::POST, API.to_string(), Some(&body))
            .await?;
        Ok(Machine {
            native_id: http::text(VENDOR, &operation, &operation_answer, "/resource_id")?,
            name: request.name.to_string(),
            instance_type: request.instance_type.to_string(),
            state: MachineState::Provisioning,
            created_at: Some(http::launch_stamp().1),
        })
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        self.call(
            &format!("delete instance {native_id}"),
            reqwest::Method::DELETE,
            format!("{API}/{native_id}"),
            None,
        )
        .await
        .map(drop)
    }

    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError> {
        let operation = format!("read instance {native_id}");
        match self
            .call(&operation, reqwest::Method::GET, format!("{API}/{native_id}"), None)
            .await
        {
            Ok(instance) => machine(&operation, &instance).map(Some),
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list instances";
        let project = required_setting(VENDOR, "project-id")?;
        let mut machines = Vec::new();
        let mut page = String::new();
        loop {
            let answer = self
                .call(
                    operation,
                    reqwest::Method::GET,
                    format!("{API}?parent_id={project}&page_token={page}"),
                    None,
                )
                .await?;
            for instance in answer.get("items").and_then(Value::as_array).into_iter().flatten() {
                machines.push(machine(operation, instance)?);
            }
            match http::optional_text(&answer, "/next_page_token") {
                Some(next) => page = next,
                None => return Ok(machines),
            }
        }
    }
}
