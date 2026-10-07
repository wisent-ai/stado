//! Lambda Cloud on-demand instances.
//!
//! API: <https://docs.lambda.ai/api/cloud> (OpenAPI `spec.json`). Bearer API
//! key; `POST /instance-operations/launch` takes `region_name`,
//! `instance_type_name`, exactly one `ssh_key_names` entry, `hostname`,
//! `name`, cloud-init `user_data` and `tags`; `POST
//! /instance-operations/terminate` takes `instance_ids`; `GET /instances` and
//! `GET /instances/{id}` read them. Lambda reports no creation time, so the
//! launch time travels as the `stado-launched-at` tag. A refusal carries
//! `error.code`; `instance-operations/launch/insufficient-capacity` and
//! `global/quota-exceeded` are capacity.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret, setting};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http::{self, LAUNCHED_AT};
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::Lambda;
const API: &str = "https://cloud.lambda.ai/api/v1";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent Lambda Cloud on-demand GPU instances and run an agent on each.",
    config: &[
        ConfigField::scalar("region", "LAMBDA_REGION", "lambda.region").required(),
        ConfigField::scalar("ssh-key-name", "LAMBDA_SSH_KEY_NAME", "lambda.ssh_key_name")
            .required(),
        ConfigField::scalar("image-family", "LAMBDA_IMAGE_FAMILY", "lambda.image_family"),
    ],
    credential_fields: &["api_key"],
    offers: &[
        ("gpu_1x_a10", "nvidia-a10"),
        ("gpu_1x_rtx6000", "nvidia-rtx6000-24gb"),
        ("gpu_1x_a100_sxm4", "nvidia-tesla-a100"),
        ("gpu_1x_a100", "nvidia-tesla-a100"),
        ("gpu_1x_a6000", "nvidia-rtx-a6000-48gb"),
        ("gpu_1x_h100_pcie", "nvidia-h100-80gb"),
        ("gpu_1x_h100_sxm5", "nvidia-h100-80gb"),
        ("gpu_8x_a100_80gb_sxm4", "nvidia-a100-80gb"),
        ("gpu_8x_v100", "nvidia-tesla-v100-16gb"),
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

    async fn call(
        &self,
        operation: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, GpuCloudError> {
        let key = secret(VENDOR, "api_key").await?;
        let mut request = self
            .client
            .request(method, format!("{API}{path}"))
            .bearer_auth(key);
        if let Some(body) = body {
            request = request.json(body);
        }
        http::exchange(VENDOR, operation, request).await
    }
}

fn state(raw: &str) -> Result<MachineState, String> {
    match raw {
        "booting" => Ok(MachineState::Provisioning),
        "active" | "unhealthy" => Ok(MachineState::Running),
        "terminating" => Ok(MachineState::Terminating),
        "terminated" | "preempted" => Ok(MachineState::Terminated),
        other => Err(format!(
            "instance status {other:?} is not one Lambda documents"
        )),
    }
}

fn machine(operation: &str, value: &Value) -> Result<Machine, GpuCloudError> {
    let status = http::text(VENDOR, operation, value, "/status")?;
    let launched = value
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|tag| tag.get("key").and_then(Value::as_str) == Some(LAUNCHED_AT))
        .and_then(|tag| tag.get("value").and_then(Value::as_str))
        .and_then(http::parse_time);
    Ok(Machine {
        native_id: http::text(VENDOR, operation, value, "/id")?,
        name: http::optional_text(value, "/name").unwrap_or_default(),
        instance_type: http::text(VENDOR, operation, value, "/instance_type/name")?,
        state: state(&status)
            .map_err(|detail| GpuCloudError::response(VENDOR, operation, detail))?,
        created_at: launched,
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("launch {}", request.instance_type);
        let (stamp, launched_at) = http::launch_stamp();
        let mut body = json!({
            "region_name": required_setting(VENDOR, "region")?,
            "instance_type_name": request.instance_type,
            "ssh_key_names": [required_setting(VENDOR, "ssh-key-name")?],
            "name": request.name,
            "hostname": request.name,
            "user_data": request.startup_script,
            "tags": [{"key": LAUNCHED_AT, "value": stamp}],
        });
        let family = setting(VENDOR, "image-family")?;
        if !family.is_empty() {
            body["image"] = json!({ "family": family });
        }
        let answer = match self
            .call(
                &operation,
                reqwest::Method::POST,
                "/instance-operations/launch",
                Some(&body),
            )
            .await
        {
            Ok(answer) => answer,
            Err(error) => {
                let code = http::refusal_member(&error, "/error/code");
                return Err(match code.as_deref() {
                    Some("instance-operations/launch/insufficient-capacity")
                    | Some("global/quota-exceeded") => GpuCloudError::capacity(
                        VENDOR,
                        request.instance_type,
                        error.detail().to_string(),
                    ),
                    _ => error,
                });
            }
        };
        let id = http::text(VENDOR, &operation, &answer, "/data/instance_ids/0")?;
        Ok(Machine {
            native_id: id,
            name: request.name.to_string(),
            instance_type: request.instance_type.to_string(),
            state: MachineState::Provisioning,
            created_at: Some(launched_at),
        })
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        let body = json!({ "instance_ids": [native_id] });
        self.call(
            &format!("terminate {native_id}"),
            reqwest::Method::POST,
            "/instance-operations/terminate",
            Some(&body),
        )
        .await
        .map(drop)
    }

    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError> {
        let operation = format!("read instance {native_id}");
        match self
            .call(
                &operation,
                reqwest::Method::GET,
                &format!("/instances/{native_id}"),
                None,
            )
            .await
        {
            Ok(answer) => {
                let data = answer.get("data").ok_or_else(|| {
                    GpuCloudError::response(VENDOR, &operation, format!("no data in {answer}"))
                })?;
                machine(&operation, data).map(Some)
            }
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list instances";
        let answer = self
            .call(operation, reqwest::Method::GET, "/instances", None)
            .await?;
        answer
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                GpuCloudError::response(VENDOR, operation, format!("no data in {answer}"))
            })?
            .iter()
            .map(|instance| machine(operation, instance))
            .collect()
    }
}
