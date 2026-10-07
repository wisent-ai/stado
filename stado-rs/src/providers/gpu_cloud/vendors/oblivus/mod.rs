//! Oblivus Cloud virtual machines.
//!
//! API: <https://documenter.getpostman.com/view/21699896/UzBtoQ3e> (v2, key
//! in the `apiKey` header). `POST /cloud/virtualserver/deploy/` takes a form
//! with `name`, `flavor`, `location`, `OS`, `OSName`, `authentication`,
//! `sshpublickey` and repeated `runcmd[]` bash commands, and answers
//! `data.instanceID`; `GET /cloud/virtualserver/delete/?vmID=` releases one;
//! `GET /cloud/virtualserver/list/` lists them with `billing.lifetime` in
//! minutes, from which the creation time is computed. Oblivus answers every
//! refusal with HTTP 400 and prose, so absence is read from the list, not
//! from a refusal.

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::Value;

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::Oblivus;
const API: &str = "https://api.oblivus.com/v2/cloud/virtualserver";
/// Where the first boot command writes the startup script before running it.
const SCRIPT_PATH: &str = "/var/lib/stado-agent-startup.sh";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent Oblivus Cloud GPU virtual machines and run an agent on each.",
    config: &[
        ConfigField::scalar("location", "OBLIVUS_LOCATION", "oblivus.location").required(),
        ConfigField::scalar("os-label", "OBLIVUS_OS_LABEL", "oblivus.os_label").required(),
        ConfigField::scalar("os-name", "OBLIVUS_OS_NAME", "oblivus.os_name").required(),
        ConfigField::scalar("ssh-public-key", "OBLIVUS_SSH_PUBLIC_KEY", "oblivus.ssh_public_key")
            .required(),
    ],
    credential_fields: &["api_key"],
    offers: &[
        ("RTX_4090_x1", "nvidia-rtx-4090-24gb"),
        ("RTX_A6000_x1", "nvidia-rtx-a6000-48gb"),
        ("L40_x1", "nvidia-l40-48gb"),
        ("A100_PCIE_80GB_x1", "nvidia-a100-80gb"),
        ("H100_PCIE_80GB_x1", "nvidia-h100-80gb"),
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

    async fn send(
        &self,
        operation: &str,
        request: reqwest::RequestBuilder,
    ) -> Result<Value, GpuCloudError> {
        let key = secret(VENDOR, "api_key").await?;
        let answer = http::exchange(VENDOR, operation, request.header("apiKey", key)).await?;
        if http::optional_text(&answer, "/status").as_deref() != Some("SUCCESS") {
            return Err(GpuCloudError::response(VENDOR, operation, answer.to_string()));
        }
        Ok(answer)
    }
}

fn machine(operation: &str, vm: &Value) -> Result<Machine, GpuCloudError> {
    let state = match http::text(VENDOR, operation, vm, "/status")?.as_str() {
        "Running" => MachineState::Running,
        "Stopped" => MachineState::Stopped,
        _ => MachineState::Provisioning,
    };
    let created_at = http::optional_text(vm, "/billing/lifetime")
        .and_then(|minutes| minutes.parse::<i64>().ok())
        .map(|minutes| chrono::Utc::now() - chrono::TimeDelta::minutes(minutes));
    Ok(Machine {
        native_id: http::text(VENDOR, operation, vm, "/ID")?,
        name: http::optional_text(vm, "/name").unwrap_or_default(),
        instance_type: http::text(VENDOR, operation, vm, "/resources/flavor")?,
        state,
        created_at,
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("deploy VM {}", request.instance_type);
        let encoded = base64::engine::general_purpose::STANDARD.encode(request.startup_script);
        let form = vec![
            ("name", request.name.to_string()),
            ("flavor", request.instance_type.to_string()),
            ("location", required_setting(VENDOR, "location")?),
            ("OS", required_setting(VENDOR, "os-label")?),
            ("OSName", required_setting(VENDOR, "os-name")?),
            ("authentication", "sshpublickey".to_string()),
            ("sshpublickey", required_setting(VENDOR, "ssh-public-key")?),
            ("runcmd[]", format!("echo {encoded} | base64 -d > {SCRIPT_PATH}")),
            ("runcmd[]", format!("bash {SCRIPT_PATH}")),
        ];
        let answer = self
            .send(
                &operation,
                self.client.post(format!("{API}/deploy/")).form(&form),
            )
            .await?;
        Ok(Machine {
            native_id: http::text(VENDOR, &operation, &answer, "/data/instanceID")?,
            name: request.name.to_string(),
            instance_type: request.instance_type.to_string(),
            state: MachineState::Provisioning,
            created_at: Some(http::launch_stamp().1),
        })
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        let operation = format!("delete VM {native_id}");
        if self.machine(native_id).await?.is_none() {
            return Err(GpuCloudError::NotFound {
                vendor: VENDOR.display_name(),
                operation,
                detail: "the VM list does not carry it".to_string(),
            });
        }
        self.send(
            &operation,
            self.client
                .get(format!("{API}/delete/"))
                .query(&[("vmID", native_id)]),
        )
        .await
        .map(drop)
    }

    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError> {
        Ok(self
            .machines()
            .await?
            .into_iter()
            .find(|machine| machine.native_id == native_id))
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list VMs";
        let answer = self
            .send(operation, self.client.get(format!("{API}/list/")))
            .await?;
        answer
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| GpuCloudError::response(VENDOR, operation, format!("no data in {answer}")))?
            .iter()
            .map(|vm| machine(operation, vm))
            .collect()
    }
}
