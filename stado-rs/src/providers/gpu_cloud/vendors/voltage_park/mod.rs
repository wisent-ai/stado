//! Voltage Park on-demand virtual machines.
//!
//! API: <https://cloud-api.voltagepark.com/api/v1/redoc> (OpenAPI
//! `openapi.json`). Bearer API token. A VM is deployed from an instant
//! preset: `GET /virtual-machines/instant/locations` lists, per location,
//! the presets with their `resources.gpus` and `available_vms`; `POST
//! /virtual-machines/instant` takes the preset's `config_id`, `name`,
//! `organization_ssh_keys`, `cloud_init` (`write_files`, `runcmd`) and `tags`
//! and answers `vm_id`. `DELETE /virtual-machines/{id}` releases it, `GET
//! /virtual-machines/{id}` and the offset-paged `GET /virtual-machines/` read
//! them. The instance type is the GPU key (`h100-sxm5-80gb`); no preset with
//! an available VM carrying it is capacity.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{secret, setting};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::VoltagePark;
const API: &str = "https://cloud-api.voltagepark.com/api/v1";
/// Where cloud-init writes the startup script before running it.
const SCRIPT_PATH: &str = "/var/lib/stado-agent-startup.sh";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent Voltage Park on-demand GPU virtual machines and run an agent on each.",
    config: &[ConfigField::scalar(
        "location",
        "VOLTAGE_PARK_LOCATION",
        "voltage-park.location",
    )],
    credential_fields: &["api_key"],
    offers: &[
        ("h100-sxm5-80gb", "nvidia-h100-80gb"),
        ("h200-sxm5-141gb", "nvidia-h200-141gb"),
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

    /// The first preset, in the configured location when one is set, whose
    /// GPUs are `gpu` and that still has a VM to give.
    async fn preset(&self, gpu: &str) -> Result<Option<String>, GpuCloudError> {
        let operation = "list instant locations";
        let location = setting(VENDOR, "location")?;
        let answer = self
            .call(
                operation,
                reqwest::Method::GET,
                "/virtual-machines/instant/locations",
                None,
            )
            .await?;
        let locations = answer
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                GpuCloudError::response(VENDOR, operation, format!("no results in {answer}"))
            })?;
        for place in locations {
            if !location.is_empty()
                && http::optional_text(place, "/id").as_deref() != Some(location.as_str())
            {
                continue;
            }
            for preset in place
                .get("available_presets")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let carries = preset
                    .pointer("/resources/gpus")
                    .and_then(Value::as_object)
                    .is_some_and(|gpus| gpus.contains_key(gpu));
                let available = preset
                    .get("available_vms")
                    .and_then(Value::as_i64)
                    .is_some_and(i64::is_positive);
                if carries && available {
                    return Ok(Some(http::text(VENDOR, operation, preset, "/id")?));
                }
            }
        }
        Ok(None)
    }
}

fn machine(operation: &str, vm: &Value) -> Result<Machine, GpuCloudError> {
    let state = match http::text(VENDOR, operation, vm, "/status")?.as_str() {
        "Running" => MachineState::Running,
        "Stopped" | "StoppedDisassociated" | "Outbid" => MachineState::Stopped,
        "Terminated" => MachineState::Terminated,
        "Relocating" => MachineState::Provisioning,
        other => {
            return Err(GpuCloudError::response(
                VENDOR,
                operation,
                format!("VM status {other:?} is not one Voltage Park documents"),
            ))
        }
    };
    let gpu = vm
        .pointer("/resources/gpus")
        .and_then(Value::as_object)
        .and_then(|gpus| gpus.keys().next().cloned())
        .ok_or_else(|| GpuCloudError::response(VENDOR, operation, format!("no GPU in {vm}")))?;
    Ok(Machine {
        native_id: http::text(VENDOR, operation, vm, "/id")?,
        name: http::optional_text(vm, "/name").unwrap_or_default(),
        instance_type: gpu,
        state,
        created_at: http::timestamp(vm, "/timestamp_creation"),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("deploy instant VM {}", request.instance_type);
        let Some(config_id) = self.preset(request.instance_type).await? else {
            return Err(GpuCloudError::capacity(
                VENDOR,
                request.instance_type,
                "no instant preset with an available VM carries this GPU",
            ));
        };
        let body = json!({
            "config_id": config_id,
            "name": request.name,
            "organization_ssh_keys": {"mode": "all"},
            "cloud_init": {
                "write_files": [{
                    "path": SCRIPT_PATH,
                    "content": request.startup_script,
                    "permissions": "0700",
                }],
                "runcmd": [format!("bash {SCRIPT_PATH}")],
            },
            "tags": ["stado-agent"],
        });
        let answer = self
            .call(
                &operation,
                reqwest::Method::POST,
                "/virtual-machines/instant",
                Some(&body),
            )
            .await?;
        Ok(Machine {
            native_id: http::text(VENDOR, &operation, &answer, "/vm_id")?,
            name: request.name.to_string(),
            instance_type: request.instance_type.to_string(),
            state: MachineState::Provisioning,
            created_at: Some(http::launch_stamp().1),
        })
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        self.call(
            &format!("delete VM {native_id}"),
            reqwest::Method::DELETE,
            &format!("/virtual-machines/{native_id}"),
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
                &format!("/virtual-machines/{native_id}"),
                None,
            )
            .await
        {
            Ok(vm) => machine(&operation, &vm).map(Some),
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list VMs";
        let mut machines = Vec::new();
        loop {
            let answer = self
                .call(
                    operation,
                    reqwest::Method::GET,
                    &format!("/virtual-machines/?offset={}", machines.len()),
                    None,
                )
                .await?;
            let results = answer
                .get("results")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    GpuCloudError::response(VENDOR, operation, format!("no results in {answer}"))
                })?;
            for vm in results {
                machines.push(machine(operation, vm)?);
            }
            if results.is_empty() || answer.get("has_next") != Some(&Value::Bool(true)) {
                return Ok(machines);
            }
        }
    }
}
