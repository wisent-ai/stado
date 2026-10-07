//! Arkane Cloud compute instances.
//!
//! API: <https://docs.arkanecloud.com/arkane-cloud/api-reference/api-reference/compute-provisioning>
//! and `.../compute-instances` (key in `x-api-key`, base
//! `https://console.arkanecloud.com`). `POST /api/compute/deploy` takes
//! `instanceType`, `name`, `region`, `sshKeyId`, `osVolumeSize` and
//! `osImageId` — and no startup script, so no agent can boot on a machine
//! rented there. Stado therefore lists (`GET /api/compute/instances`), reads
//! (`GET /api/compute/instances/{id}`) and releases (`DELETE
//! /api/compute/instances/{id}`) Arkane machines, and the coordinator never
//! dispatches to Arkane; `launch` exists only to state that refusal.

use crate::capabilities::GpuCloudVendor;
use crate::providers::gpu_cloud::access::secret;
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};
use async_trait::async_trait;
use serde_json::Value;

const VENDOR: GpuCloudVendor = GpuCloudVendor::Arkane;
const API: &str = "https://console.arkanecloud.com/api/compute";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "List, read and release Arkane Cloud GPU instances; its API boots no agent.",
    config: &[],
    credential_fields: &["api_key"],
    offers: &[
        ("H100.1x", "nvidia-h100-80gb"),
        ("H200.1x", "nvidia-h200-141gb"),
    ],
    guest: GuestIdentity::NoStartupScript,
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
    ) -> Result<Value, GpuCloudError> {
        let key = secret(VENDOR, "api_key").await?;
        let request = self
            .client
            .request(method, format!("{API}{path}"))
            .header("x-api-key", key);
        http::exchange(VENDOR, operation, request).await
    }
}

fn machine(operation: &str, instance: &Value) -> Result<Machine, GpuCloudError> {
    let state = match http::text(VENDOR, operation, instance, "/status")?.as_str() {
        "running" => MachineState::Running,
        "stopping" => MachineState::Stopping,
        "stopped" | "suspended" | "hibernated" => MachineState::Stopped,
        "terminating" | "deleting" => MachineState::Terminating,
        "terminated" | "deleted" => MachineState::Terminated,
        "error" | "failed" => MachineState::Failed,
        _ => MachineState::Provisioning,
    };
    Ok(Machine {
        native_id: http::text(VENDOR, operation, instance, "/id")?,
        name: http::optional_text(instance, "/name").unwrap_or_default(),
        instance_type: http::text(VENDOR, operation, instance, "/type")?,
        state,
        created_at: http::timestamp(instance, "/createdAt"),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        Err(GpuCloudError::Configuration(format!(
            "Arkane Cloud: POST /api/compute/deploy takes no startup script, so {} cannot boot \
             a Stado agent",
            request.name
        )))
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        self.call(
            &format!("delete instance {native_id}"),
            reqwest::Method::DELETE,
            &format!("/instances/{native_id}"),
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
        let answer = self
            .call(operation, reqwest::Method::GET, "/instances")
            .await?;
        answer
            .as_array()
            .ok_or_else(|| {
                GpuCloudError::response(VENDOR, operation, format!("not a list: {answer}"))
            })?
            .iter()
            .map(|instance| machine(operation, instance))
            .collect()
    }
}
