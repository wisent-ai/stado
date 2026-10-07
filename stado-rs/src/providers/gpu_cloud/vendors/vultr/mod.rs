//! Vultr Cloud GPU instances.
//!
//! API: <https://www.vultr.com/api/> (v2). Bearer API key. `POST /instances`
//! takes `region`, `plan`, `os_id`, `label`, `hostname`, base64 `user_data`
//! and `sshkey_id`; `DELETE /instances/{id}` releases one; `GET
//! /instances/{id}` and the cursor-paged `GET /instances` read them, with
//! `date_created` as the creation time. Vultr names no capacity code in a
//! refusal, so the adapter asks `GET /regions/{region}/availability?type=vcg`
//! first: a plan the region does not list as available is capacity.

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret, setting};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::Vultr;
const API: &str = "https://api.vultr.com/v2";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent Vultr Cloud GPU instances and run an agent on each.",
    config: &[
        ConfigField::scalar("region", "VULTR_REGION", "vultr.region").required(),
        ConfigField::scalar("os-id", "VULTR_OS_ID", "vultr.os_id").required(),
        ConfigField::scalar("ssh-key-id", "VULTR_SSH_KEY_ID", "vultr.ssh_key_id"),
    ],
    credential_fields: &["api_key"],
    offers: &[
        ("vcg-a40-8c-40g-16vram", "nvidia-a40-16gb"),
        ("vcg-a40-12c-60g-24vram", "nvidia-a40-24gb"),
        ("vcg-l40s-16c-180g-48vram", "nvidia-l40s"),
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
        url: &str,
        body: Option<&Value>,
    ) -> Result<Value, GpuCloudError> {
        let key = secret(VENDOR, "api_key").await?;
        let mut request = self.client.request(method, url).bearer_auth(key);
        if let Some(body) = body {
            request = request.json(body);
        }
        http::exchange(VENDOR, operation, request).await
    }
}

fn state(instance: &Value) -> MachineState {
    let status = instance.get("status").and_then(Value::as_str);
    let power = instance.get("power_status").and_then(Value::as_str);
    match (status, power) {
        (Some("pending"), _) => MachineState::Provisioning,
        (Some("suspended"), _) | (_, Some("stopped")) => MachineState::Stopped,
        _ => MachineState::Running,
    }
}

fn machine(operation: &str, instance: &Value) -> Result<Machine, GpuCloudError> {
    Ok(Machine {
        native_id: http::text(VENDOR, operation, instance, "/id")?,
        name: http::optional_text(instance, "/label").unwrap_or_default(),
        instance_type: http::text(VENDOR, operation, instance, "/plan")?,
        state: state(instance),
        created_at: http::timestamp(instance, "/date_created"),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("launch {}", request.instance_type);
        let region = required_setting(VENDOR, "region")?;
        let os = required_setting(VENDOR, "os-id")?;
        let os_id: i64 = os.parse().map_err(|_| {
            GpuCloudError::Configuration(format!(
                "Vultr: vultr.os_id must be the numeric operating-system id `GET /v2/os` lists, \
                 not {os:?}"
            ))
        })?;
        let availability = self
            .call(
                &format!("read availability in {region}"),
                reqwest::Method::GET,
                &format!("{API}/regions/{region}/availability?type=vcg"),
                None,
            )
            .await?;
        let available = availability
            .get("available_plans")
            .and_then(Value::as_array)
            .is_some_and(|plans| {
                plans
                    .iter()
                    .any(|plan| plan.as_str() == Some(request.instance_type))
            });
        if !available {
            return Err(GpuCloudError::capacity(
                VENDOR,
                request.instance_type,
                format!("region {region} lists {availability} as available"),
            ));
        }
        let mut body = json!({
            "region": region,
            "plan": request.instance_type,
            "os_id": os_id,
            "label": request.name,
            "hostname": request.name,
            "user_data": base64::engine::general_purpose::STANDARD.encode(request.startup_script),
        });
        let key = setting(VENDOR, "ssh-key-id")?;
        if !key.is_empty() {
            body["sshkey_id"] = json!([key]);
        }
        let answer = self
            .call(
                &operation,
                reqwest::Method::POST,
                &format!("{API}/instances"),
                Some(&body),
            )
            .await?;
        let instance = answer.get("instance").ok_or_else(|| {
            GpuCloudError::response(VENDOR, &operation, format!("no instance in {answer}"))
        })?;
        machine(&operation, instance)
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        self.call(
            &format!("delete instance {native_id}"),
            reqwest::Method::DELETE,
            &format!("{API}/instances/{native_id}"),
            None,
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
                &format!("{API}/instances/{native_id}"),
                None,
            )
            .await
        {
            Ok(answer) => {
                let instance = answer.get("instance").ok_or_else(|| {
                    GpuCloudError::response(VENDOR, &operation, format!("no instance in {answer}"))
                })?;
                machine(&operation, instance).map(Some)
            }
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list instances";
        let mut machines = Vec::new();
        let mut url = format!("{API}/instances");
        loop {
            let answer = self
                .call(operation, reqwest::Method::GET, &url, None)
                .await?;
            for instance in answer
                .get("instances")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    GpuCloudError::response(VENDOR, operation, format!("no instances in {answer}"))
                })?
            {
                machines.push(machine(operation, instance)?);
            }
            match http::optional_text(&answer, "/meta/links/next") {
                Some(cursor) => url = format!("{API}/instances?cursor={cursor}"),
                None => return Ok(machines),
            }
        }
    }
}
