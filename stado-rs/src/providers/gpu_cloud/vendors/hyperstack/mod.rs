//! Hyperstack virtual machines.
//!
//! API: <https://docs.hyperstack.cloud/docs/api-reference/introduction>. The
//! key travels in the `api_key` header. `POST /core/virtual-machines` takes
//! `name`, `environment_name`, `flavor_name`, `image_name`, `key_name`,
//! `count`, `user_data` and `assign_floating_ip`, and answers with the
//! scheduled `instances`; `DELETE /core/virtual-machines/{id}` releases one;
//! `GET /core/virtual-machines/{id}` and `GET /core/virtual-machines` read
//! them. A status the vendor adds later than this adapter reads as
//! provisioning, so a new transitional state never hides a billed machine.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::Hyperstack;
const API: &str = "https://infrahub-api.nexgencloud.com/v1";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent Hyperstack GPU virtual machines and run an agent on each.",
    config: &[
        ConfigField::scalar("environment", "HYPERSTACK_ENVIRONMENT", "hyperstack.environment")
            .required(),
        ConfigField::scalar("image", "HYPERSTACK_IMAGE", "hyperstack.image").required(),
        ConfigField::scalar("key-name", "HYPERSTACK_KEY_NAME", "hyperstack.key_name").required(),
    ],
    credential_fields: &["api_key"],
    offers: &[
        ("n3-RTX-A6000x1", "nvidia-rtx-a6000-48gb"),
        ("n3-L40x1", "nvidia-l40-48gb"),
        ("n3-A100x1", "nvidia-a100-80gb"),
        ("n3-H100x1", "nvidia-h100-80gb"),
        ("n3-H100-SXM5x8", "nvidia-h100-80gb"),
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
            .header("api_key", key);
        if let Some(body) = body {
            request = request.json(body);
        }
        http::exchange(VENDOR, operation, request).await
    }
}

fn state(raw: &str) -> MachineState {
    match raw {
        "ACTIVE" => MachineState::Running,
        "SHUTOFF" | "HIBERNATED" | "HIBERNATING" | "STOPPED" => MachineState::Stopped,
        "DELETING" => MachineState::Terminating,
        "DELETED" => MachineState::Terminated,
        "ERROR" => MachineState::Failed,
        _ => MachineState::Provisioning,
    }
}

fn machine(operation: &str, instance: &Value) -> Result<Machine, GpuCloudError> {
    Ok(Machine {
        native_id: http::text(VENDOR, operation, instance, "/id")?,
        name: http::optional_text(instance, "/name").unwrap_or_default(),
        instance_type: http::text(VENDOR, operation, instance, "/flavor/name")?,
        state: state(&http::text(VENDOR, operation, instance, "/status")?),
        created_at: http::timestamp(instance, "/created_at"),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("create virtual machine {}", request.instance_type);
        let body = json!({
            "name": request.name,
            "environment_name": required_setting(VENDOR, "environment")?,
            "flavor_name": request.instance_type,
            "image_name": required_setting(VENDOR, "image")?,
            "key_name": required_setting(VENDOR, "key-name")?,
            "count": std::iter::once(request).count(),
            "user_data": request.startup_script,
            "assign_floating_ip": false,
        });
        let answer = self
            .call(&operation, reqwest::Method::POST, "/core/virtual-machines", Some(&body))
            .await?;
        let instance = answer.pointer("/instances/0").ok_or_else(|| {
            GpuCloudError::response(VENDOR, &operation, format!("no instance in {answer}"))
        })?;
        let mut launched = machine(&operation, instance)?;
        if launched.created_at.is_none() {
            launched.created_at = Some(http::launch_stamp().1);
        }
        Ok(launched)
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        self.call(
            &format!("delete virtual machine {native_id}"),
            reqwest::Method::DELETE,
            &format!("/core/virtual-machines/{native_id}"),
            None,
        )
        .await
        .map(drop)
    }

    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError> {
        let operation = format!("read virtual machine {native_id}");
        match self
            .call(&operation, reqwest::Method::GET, &format!("/core/virtual-machines/{native_id}"), None)
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
        let operation = "list virtual machines";
        let answer = self
            .call(operation, reqwest::Method::GET, "/core/virtual-machines", None)
            .await?;
        answer
            .get("instances")
            .and_then(Value::as_array)
            .ok_or_else(|| GpuCloudError::response(VENDOR, operation, format!("no instances in {answer}")))?
            .iter()
            .map(|instance| machine(operation, instance))
            .collect()
    }
}
