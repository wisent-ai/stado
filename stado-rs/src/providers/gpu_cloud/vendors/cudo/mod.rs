//! Cudo Compute virtual machines.
//!
//! API: <https://docs.cudocompute.com/api> (Bearer API key, base
//! `https://rest.compute.cudo.org`). `POST /v1/projects/{projectId}/vm`
//! creates and starts a VM from `vmId`, `dataCenterId`, `machineType`,
//! `gpus`, `vcpus`, `memoryGib`, `bootDiskImageId`, `bootDiskSizeGib` and
//! `startScript`; `POST /v1/projects/{projectId}/vms/{id}/terminate`
//! releases it; `GET /v1/projects/{projectId}/vms/{id}` and `GET
//! /v1/projects/{projectId}/vms` read them. Cudo takes the VM id from the
//! caller, so the instance name is the id.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::Cudo;
const API: &str = "https://rest.compute.cudo.org/v1/projects";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent Cudo Compute GPU virtual machines and run an agent on each.",
    config: &[
        ConfigField::scalar("project-id", "CUDO_PROJECT_ID", "cudo.project_id").required(),
        ConfigField::scalar("data-center", "CUDO_DATA_CENTER", "cudo.data_center").required(),
        ConfigField::scalar("image-id", "CUDO_IMAGE_ID", "cudo.image_id").required(),
        ConfigField::scalar("vcpus", "CUDO_VCPUS", "cudo.vcpus").required(),
        ConfigField::scalar("memory-gib", "CUDO_MEMORY_GIB", "cudo.memory_gib").required(),
    ],
    credential_fields: &["api_key"],
    offers: &[
        ("epyc-milan-rtx-a6000", "nvidia-rtx-a6000-48gb"),
        ("epyc-milan-a100-pcie", "nvidia-tesla-a100"),
        ("sapphire-rapids-h100", "nvidia-h100-80gb"),
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
        let project = required_setting(VENDOR, "project-id")?;
        let key = secret(VENDOR, "api_key").await?;
        let mut request = self
            .client
            .request(method, format!("{API}/{project}{path}"))
            .bearer_auth(key);
        if let Some(body) = body {
            request = request.json(body);
        }
        http::exchange(VENDOR, operation, request).await
    }
}

/// A whole-number setting.
fn count(key: &str) -> Result<u32, GpuCloudError> {
    let raw = required_setting(VENDOR, key)?;
    raw.parse().map_err(|_| {
        GpuCloudError::Configuration(format!(
            "Cudo Compute: setting {key} must be a whole number, not {raw:?}"
        ))
    })
}

fn state(raw: &str) -> MachineState {
    match raw {
        "ACTIVE" => MachineState::Running,
        "STOPPING" | "SUSPENDING" => MachineState::Stopping,
        "STOPPED" | "SUSPENDED" => MachineState::Stopped,
        "DELETING" => MachineState::Terminating,
        "DELETED" => MachineState::Terminated,
        "FAILED" => MachineState::Failed,
        _ => MachineState::Provisioning,
    }
}

fn machine(operation: &str, vm: &Value) -> Result<Machine, GpuCloudError> {
    let id = http::text(VENDOR, operation, vm, "/id")?;
    Ok(Machine {
        native_id: id.clone(),
        name: id,
        instance_type: http::text(VENDOR, operation, vm, "/machineType")?,
        state: state(&http::text(VENDOR, operation, vm, "/state")?),
        created_at: http::timestamp(vm, "/createTime"),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("create VM {}", request.instance_type);
        let body = json!({
            "vmId": request.name,
            "dataCenterId": required_setting(VENDOR, "data-center")?,
            "machineType": request.instance_type,
            "gpus": std::iter::once(request).count(),
            "vcpus": count("vcpus")?,
            "memoryGib": count("memory-gib")?,
            "bootDiskImageId": required_setting(VENDOR, "image-id")?,
            "bootDiskSizeGib": request.boot_disk_gb,
            "startScript": request.startup_script,
        });
        let answer = self
            .call(&operation, reqwest::Method::POST, "/vm", Some(&body))
            .await?;
        match answer.get("vm") {
            Some(vm) => machine(&operation, vm),
            None => Ok(Machine {
                native_id: http::text(VENDOR, &operation, &answer, "/id")?,
                name: request.name.to_string(),
                instance_type: request.instance_type.to_string(),
                state: MachineState::Provisioning,
                created_at: Some(http::launch_stamp().1),
            }),
        }
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        self.call(
            &format!("terminate VM {native_id}"),
            reqwest::Method::POST,
            &format!("/vms/{native_id}/terminate"),
            None,
        )
        .await
        .map(drop)
    }

    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError> {
        let operation = format!("read VM {native_id}");
        match self
            .call(
                &operation,
                reqwest::Method::GET,
                &format!("/vms/{native_id}"),
                None,
            )
            .await
        {
            Ok(answer) => {
                let vm = answer.get("VM").ok_or_else(|| {
                    GpuCloudError::response(VENDOR, &operation, format!("no VM in {answer}"))
                })?;
                machine(&operation, vm).map(Some)
            }
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list VMs";
        let answer = self
            .call(operation, reqwest::Method::GET, "/vms", None)
            .await?;
        answer
            .get("VMs")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                GpuCloudError::response(VENDOR, operation, format!("no VMs in {answer}"))
            })?
            .iter()
            .map(|vm| machine(operation, vm))
            .collect()
    }
}
