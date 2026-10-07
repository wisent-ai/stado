//! Latitude.sh GPU virtual machines.
//!
//! API: <https://www.latitude.sh/docs/api-reference> (JSON:API, Bearer
//! token). Cloud-init is a separate record: `POST /user_data` with the
//! base64 `content` returns the `ud_…` id `POST /virtual_machines` takes as
//! `user_data`, next to `project`, `name`, `plan`, `site`, `billing`,
//! `operating_system` and `ssh_keys`. The record carries the startup script,
//! so `terminate` deletes it together with the VM (`DELETE
//! /virtual_machines/{id}`, then `DELETE /user_data/{id}` for the record
//! described by the VM's name). Statuses are an open set; one this adapter
//! does not name reads as provisioning.

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

const VENDOR: GpuCloudVendor = GpuCloudVendor::Latitude;
const API: &str = "https://api.latitude.sh";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent Latitude.sh GPU virtual machines and run an agent on each.",
    config: &[
        ConfigField::scalar("project", "LATITUDE_PROJECT", "latitude.project").required(),
        ConfigField::scalar("site", "LATITUDE_SITE", "latitude.site").required(),
        ConfigField::scalar(
            "operating-system",
            "LATITUDE_OPERATING_SYSTEM",
            "latitude.operating_system",
        ),
        ConfigField::scalar("ssh-key", "LATITUDE_SSH_KEY", "latitude.ssh_key"),
    ],
    credential_fields: &["api_key"],
    offers: &[
        ("g3.l40s.medium", "nvidia-l40s"),
        ("g3.h100.large", "nvidia-h100-80gb"),
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

    /// Delete the cloud-init records described by `name`.
    async fn forget_user_data(&self, name: &str) -> Result<(), GpuCloudError> {
        let operation = "list user data";
        let answer = self.call(operation, reqwest::Method::GET, "/user_data", None).await?;
        for record in answer.get("data").and_then(Value::as_array).into_iter().flatten() {
            if http::optional_text(record, "/attributes/description").as_deref() == Some(name) {
                let id = http::text(VENDOR, operation, record, "/id")?;
                self.call(
                    &format!("delete user data {id}"),
                    reqwest::Method::DELETE,
                    &format!("/user_data/{id}"),
                    None,
                )
                .await?;
            }
        }
        Ok(())
    }
}

fn state(raw: &str) -> MachineState {
    match raw {
        "Running" => MachineState::Running,
        "Stopping" => MachineState::Stopping,
        "Stopped" | "Off" => MachineState::Stopped,
        "Deleting" | "Destroying" => MachineState::Terminating,
        "Failed" | "Error" => MachineState::Failed,
        _ => MachineState::Provisioning,
    }
}

fn machine(operation: &str, vm: &Value) -> Result<Machine, GpuCloudError> {
    let plan = http::optional_text(vm, "/attributes/plan/slug")
        .or_else(|| http::optional_text(vm, "/attributes/plan/name"))
        .ok_or_else(|| GpuCloudError::response(VENDOR, operation, format!("no plan in {vm}")))?;
    Ok(Machine {
        native_id: http::text(VENDOR, operation, vm, "/id")?,
        name: http::optional_text(vm, "/attributes/name").unwrap_or_default(),
        instance_type: plan,
        state: state(&http::text(VENDOR, operation, vm, "/attributes/status")?),
        created_at: http::timestamp(vm, "/attributes/created_at"),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("create virtual machine {}", request.instance_type);
        let project = required_setting(VENDOR, "project")?;
        let site = required_setting(VENDOR, "site")?;
        let record = self
            .call(
                &format!("create user data for {}", request.name),
                reqwest::Method::POST,
                "/user_data",
                Some(&json!({"data": {"type": "user_data", "attributes": {
                    "description": request.name,
                    "content": base64::engine::general_purpose::STANDARD.encode(request.startup_script),
                }}})),
            )
            .await?;
        let user_data = http::text(VENDOR, &operation, &record, "/data/id")?;
        let mut attributes = json!({
            "project": project,
            "name": request.name,
            "plan": request.instance_type,
            "site": site,
            "billing": "hourly",
            "user_data": user_data,
        });
        let os = setting(VENDOR, "operating-system")?;
        if !os.is_empty() {
            attributes["operating_system"] = json!(os);
        }
        let key = setting(VENDOR, "ssh-key")?;
        if !key.is_empty() {
            attributes["ssh_keys"] = json!([key]);
        }
        let answer = self
            .call(
                &operation,
                reqwest::Method::POST,
                "/virtual_machines",
                Some(&json!({"data": {"type": "virtual_machines", "attributes": attributes}})),
            )
            .await;
        let answer = match answer {
            Ok(answer) => answer,
            Err(error) => {
                self.forget_user_data(request.name).await?;
                return Err(error);
            }
        };
        let vm = answer.get("data").ok_or_else(|| {
            GpuCloudError::response(VENDOR, &operation, format!("no data in {answer}"))
        })?;
        let mut launched = machine(&operation, vm)?;
        launched.instance_type = request.instance_type.to_string();
        Ok(launched)
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        let name = self.machine(native_id).await?.map(|vm| vm.name);
        self.call(
            &format!("delete virtual machine {native_id}"),
            reqwest::Method::DELETE,
            &format!("/virtual_machines/{native_id}"),
            None,
        )
        .await?;
        match name {
            Some(name) if !name.is_empty() => self.forget_user_data(&name).await,
            _ => Ok(()),
        }
    }

    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError> {
        let operation = format!("read virtual machine {native_id}");
        match self
            .call(&operation, reqwest::Method::GET, &format!("/virtual_machines/{native_id}"), None)
            .await
        {
            Ok(answer) => {
                let vm = answer.get("data").ok_or_else(|| {
                    GpuCloudError::response(VENDOR, &operation, format!("no data in {answer}"))
                })?;
                machine(&operation, vm).map(Some)
            }
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list virtual machines";
        let answer = self
            .call(operation, reqwest::Method::GET, "/virtual_machines", None)
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
