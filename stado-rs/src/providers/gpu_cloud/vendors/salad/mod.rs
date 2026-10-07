//! SaladCloud container groups, one replica each.
//!
//! API: <https://docs.salad.com/reference/saladcloud-api> (key in the
//! `Salad-Api-Key` header, base `https://api.salad.com/api/public`). The
//! instance type is a GPU class name, resolved to its id through `GET
//! /organizations/{org}/gpu-classes`. `POST
//! /organizations/{org}/projects/{project}/containers` creates a group from
//! `name`, `container` (`image`, `resources` with `cpu`, `memory` and
//! `gpu_classes`, `command`), `replicas`, `restart_policy` and
//! `autostart_policy`; `DELETE .../containers/{name}` removes it; `GET
//! .../containers/{name}` and `GET .../containers` read them. Salad
//! addresses a group by its name, so the native id is the name, and the
//! agent inside the container takes the same name from `STADO_WORKER_NAME`.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::Salad;
const API: &str = "https://api.salad.com/api/public/organizations";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent SaladCloud GPU containers and run an agent inside each.",
    config: &[
        ConfigField::scalar("organization", "SALAD_ORGANIZATION", "salad.organization").required(),
        ConfigField::scalar("project", "SALAD_PROJECT", "salad.project").required(),
        ConfigField::scalar("image", "SALAD_IMAGE", "salad.image").required(),
        ConfigField::scalar("vcpus", "SALAD_VCPUS", "salad.vcpus").required(),
        ConfigField::scalar("memory-mb", "SALAD_MEMORY_MB", "salad.memory_mb").required(),
    ],
    credential_fields: &["api_key"],
    offers: &[
        ("RTX 3090 (24 GB)", "nvidia-rtx-3090-24gb"),
        ("RTX 4090 (24 GB)", "nvidia-rtx-4090-24gb"),
        ("RTX 5090 (32 GB)", "nvidia-rtx-5090-32gb"),
    ],
    guest: GuestIdentity::ContainerName,
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
        url: String,
        body: Option<&Value>,
    ) -> Result<Value, GpuCloudError> {
        let key = secret(VENDOR, "api_key").await?;
        let mut request = self
            .client
            .request(method, url)
            .header("Salad-Api-Key", key);
        if let Some(body) = body {
            request = request.json(body);
        }
        http::exchange(VENDOR, operation, request).await
    }

    fn containers(&self) -> Result<String, GpuCloudError> {
        Ok(format!(
            "{API}/{}/projects/{}/containers",
            required_setting(VENDOR, "organization")?,
            required_setting(VENDOR, "project")?
        ))
    }

    /// GPU class id by name, and name by id.
    async fn gpu_classes(&self) -> Result<Vec<(String, String)>, GpuCloudError> {
        let operation = "list GPU classes";
        let answer = self
            .call(
                operation,
                reqwest::Method::GET,
                format!(
                    "{API}/{}/gpu-classes",
                    required_setting(VENDOR, "organization")?
                ),
                None,
            )
            .await?;
        answer
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                GpuCloudError::response(VENDOR, operation, format!("no items in {answer}"))
            })?
            .iter()
            .map(|class| {
                Ok((
                    http::text(VENDOR, operation, class, "/id")?,
                    http::text(VENDOR, operation, class, "/name")?,
                ))
            })
            .collect()
    }
}

/// A whole-number setting.
fn count(key: &str) -> Result<u32, GpuCloudError> {
    let raw = required_setting(VENDOR, key)?;
    raw.parse().map_err(|_| {
        GpuCloudError::Configuration(format!(
            "SaladCloud: setting {key} must be a whole number, not {raw:?}"
        ))
    })
}

fn machine(
    operation: &str,
    group: &Value,
    classes: &[(String, String)],
) -> Result<Machine, GpuCloudError> {
    let state = match http::text(VENDOR, operation, group, "/current_state/status")?.as_str() {
        "running" => MachineState::Running,
        "stopped" | "succeeded" => MachineState::Stopped,
        "failed" => MachineState::Failed,
        _ => MachineState::Provisioning,
    };
    let class = http::text(
        VENDOR,
        operation,
        group,
        "/container/resources/gpu_classes/0",
    )?;
    let instance_type = classes
        .iter()
        .find(|(id, _)| *id == class)
        .map_or(class, |(_, name)| name.clone());
    let name = http::text(VENDOR, operation, group, "/name")?;
    Ok(Machine {
        native_id: name.clone(),
        name,
        instance_type,
        state,
        created_at: http::timestamp(group, "/create_time"),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("create container group on {}", request.instance_type);
        let classes = self.gpu_classes().await?;
        let Some((class, _)) = classes
            .iter()
            .find(|(_, name)| name == request.instance_type)
        else {
            return Err(GpuCloudError::Configuration(format!(
                "SaladCloud: GPU class {:?} is not offered to this organization; it offers {:?}",
                request.instance_type,
                classes.iter().map(|(_, name)| name).collect::<Vec<_>>()
            )));
        };
        let body = json!({
            "name": request.name,
            "container": {
                "image": required_setting(VENDOR, "image")?,
                "resources": {
                    "cpu": count("vcpus")?,
                    "memory": count("memory-mb")?,
                    "gpu_classes": [class],
                },
                "command": ["bash", "-c", request.startup_script],
            },
            "replicas": std::iter::once(request).count(),
            "restart_policy": "never",
            "autostart_policy": true,
        });
        let group = self
            .call(
                &operation,
                reqwest::Method::POST,
                self.containers()?,
                Some(&body),
            )
            .await?;
        let mut launched = machine(&operation, &group, &classes)?;
        if launched.created_at.is_none() {
            launched.created_at = Some(http::launch_stamp().1);
        }
        Ok(launched)
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        self.call(
            &format!("delete container group {native_id}"),
            reqwest::Method::DELETE,
            format!("{}/{native_id}", self.containers()?),
            None,
        )
        .await
        .map(drop)
    }

    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError> {
        let operation = format!("read container group {native_id}");
        let classes = self.gpu_classes().await?;
        match self
            .call(
                &operation,
                reqwest::Method::GET,
                format!("{}/{native_id}", self.containers()?),
                None,
            )
            .await
        {
            Ok(group) => machine(&operation, &group, &classes).map(Some),
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list container groups";
        let classes = self.gpu_classes().await?;
        let answer = self
            .call(operation, reqwest::Method::GET, self.containers()?, None)
            .await?;
        answer
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                GpuCloudError::response(VENDOR, operation, format!("no items in {answer}"))
            })?
            .iter()
            .map(|group| machine(operation, group, &classes))
            .collect()
    }
}
