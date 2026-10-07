//! RunPod GPU pods.
//!
//! API: <https://rest.runpod.io/v1/openapi.json>. Bearer API key. `POST /pods`
//! takes `name`, `imageName`, `gpuTypeIds`, `gpuCount`, `cloudType`,
//! `containerDiskInGb`, `dockerStartCmd`, `env` and `interruptible`;
//! `DELETE /pods/{podId}` terminates one; `GET /pods/{podId}` and `GET /pods`
//! read them. A pod is a container: the agent's startup script is its start
//! command, and the script exports `STADO_WORKER_NAME` from the
//! `RUNPOD_POD_ID` RunPod sets, so the agent publishes capacity under the pod
//! id. RunPod reports no creation time, so the launch time travels in the
//! pod's `env` as `STADO_LAUNCHED_AT`.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::Runpod;
/// The pod environment variable that carries the launch time; an
/// environment name, so it cannot be the dashed tag other vendors carry.
const LAUNCHED_ENV: &str = "STADO_LAUNCHED_AT";
const API: &str = "https://rest.runpod.io/v1";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent RunPod GPU pods and run an agent inside each pod's container.",
    config: &[
        ConfigField::scalar("image", "RUNPOD_IMAGE", "runpod.image").required(),
        ConfigField::scalar("cloud-type", "RUNPOD_CLOUD_TYPE", "runpod.cloud_type").required(),
    ],
    credential_fields: &["api_key"],
    offers: &[
        ("NVIDIA L4", "nvidia-l4"),
        ("NVIDIA GeForce RTX 4090", "nvidia-rtx-4090-24gb"),
        ("NVIDIA RTX A6000", "nvidia-rtx-a6000-48gb"),
        ("NVIDIA L40S", "nvidia-l40s"),
        ("NVIDIA A100 80GB PCIe", "nvidia-a100-80gb"),
        ("NVIDIA A100-SXM4-80GB", "nvidia-a100-80gb"),
        ("NVIDIA H100 80GB HBM3", "nvidia-h100-80gb"),
        ("NVIDIA H100 PCIe", "nvidia-h100-80gb"),
        ("AMD Instinct MI300X OAM", "amd-mi300x-192gb"),
    ],
    guest: GuestIdentity::ContainerEnv("RUNPOD_POD_ID"),
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

fn machine(operation: &str, pod: &Value) -> Result<Machine, GpuCloudError> {
    let status = http::text(VENDOR, operation, pod, "/desiredStatus")?;
    let state = match status.as_str() {
        "RUNNING" => MachineState::Running,
        "EXITED" => MachineState::Stopped,
        "TERMINATED" => MachineState::Terminated,
        other => {
            return Err(GpuCloudError::response(
                VENDOR,
                operation,
                format!("pod desiredStatus {other:?} is not one RunPod documents"),
            ))
        }
    };
    let gpu = http::optional_text(pod, "/machine/gpuTypeId")
        .or_else(|| http::optional_text(pod, "/gpu/id"))
        .unwrap_or_default();
    Ok(Machine {
        native_id: http::text(VENDOR, operation, pod, "/id")?,
        name: http::optional_text(pod, "/name").unwrap_or_default(),
        instance_type: gpu,
        state,
        created_at: pod
            .pointer("/env")
            .and_then(|env| env.get(LAUNCHED_ENV))
            .and_then(Value::as_str)
            .and_then(http::parse_time),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("create pod on {}", request.instance_type);
        let (stamp, launched_at) = http::launch_stamp();
        let body = json!({
            "name": request.name,
            "computeType": "GPU",
            "imageName": required_setting(VENDOR, "image")?,
            "cloudType": required_setting(VENDOR, "cloud-type")?,
            "gpuTypeIds": [request.instance_type],
            "gpuTypePriority": "custom",
            "interruptible": false,
            "containerDiskInGb": request.boot_disk_gb,
            "dockerStartCmd": ["bash", "-c", request.startup_script],
            "env": { LAUNCHED_ENV: stamp },
        });
        let pod = self
            .call(&operation, reqwest::Method::POST, "/pods", Some(&body))
            .await?;
        let mut machine = machine(&operation, &pod)?;
        machine.created_at = Some(launched_at);
        if machine.instance_type.is_empty() {
            machine.instance_type = request.instance_type.to_string();
        }
        Ok(machine)
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        self.call(
            &format!("terminate pod {native_id}"),
            reqwest::Method::DELETE,
            &format!("/pods/{native_id}"),
            None,
        )
        .await
        .map(drop)
    }

    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError> {
        let operation = format!("read pod {native_id}");
        match self
            .call(&operation, reqwest::Method::GET, &format!("/pods/{native_id}"), None)
            .await
        {
            Ok(pod) => machine(&operation, &pod).map(Some),
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list pods";
        let answer = self.call(operation, reqwest::Method::GET, "/pods", None).await?;
        answer
            .as_array()
            .ok_or_else(|| GpuCloudError::response(VENDOR, operation, format!("not a list: {answer}")))?
            .iter()
            .map(|pod| machine(operation, pod))
            .collect()
    }
}
