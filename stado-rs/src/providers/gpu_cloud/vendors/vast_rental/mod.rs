//! Vast.ai instances rented by Stado: the buying side of Vast.ai, apart from
//! the host listing `stado market --provider vast` sells this fleet's own
//! machine through.
//!
//! API: <https://docs.vast.ai/api-reference/openapi.yaml>. Bearer API key.
//! Vast.ai is a marketplace, so a machine is rented in two steps: `POST
//! /api/v0/bundles/` searches the offers (`gpu_name`, `num_gpus`,
//! `rentable`, `rented`, `verified`, `disk_space`, `type`, `order`) and
//! `PUT /api/v0/asks/{offer id}/` accepts the cheapest on-demand one with
//! `image`, `label`, `disk`, `runtype` `args` and `args`, answering the
//! instance id as `new_contract`. `DELETE /api/v0/instances/{id}/` destroys
//! it, `GET /api/v0/instances/{id}/` reads one (under `instances`) and the
//! keyset-paged `GET /api/v1/instances/` (`next_token`, `after_token`) reads
//! them all. An instance is a container: the startup script is its
//! entrypoint arguments (Vast.ai cuts `onstart` at 4,048 characters, which
//! an agent script exceeds), the label is the instance name and the script
//! exports it as `STADO_WORKER_NAME`. The instance type is Vast.ai's GPU
//! name (`RTX 4090`); no matching offer is capacity. `start_date` is the
//! creation time in epoch seconds.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::VastRental;
const API: &str = "https://console.vast.ai/api";

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent the cheapest verified on-demand Vast.ai offer for an accelerator and run an \
              agent inside its container.",
    config: &[ConfigField::scalar("image", "VAST_RENTAL_IMAGE", "vast-rental.image").required()],
    credential_fields: &["api_key"],
    offers: &[
        ("L4", "nvidia-l4"),
        ("RTX 4090", "nvidia-rtx-4090-24gb"),
        ("RTX A6000", "nvidia-rtx-a6000-48gb"),
        ("L40S", "nvidia-l40s"),
        ("H100 PCIE", "nvidia-h100-80gb"),
        ("H100 SXM", "nvidia-h100-80gb"),
        ("H200", "nvidia-h200-141gb"),
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
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Value, GpuCloudError> {
        let key = secret(VENDOR, "api_key").await?;
        let mut request = self
            .client
            .request(method, format!("{API}{path}"))
            .bearer_auth(key)
            .query(query);
        if let Some(body) = body {
            request = request.json(body);
        }
        http::exchange(VENDOR, operation, request).await
    }

    /// The id of the cheapest rentable, verified, on-demand offer of one GPU
    /// named `instance_type` with room for the boot disk; capacity when
    /// Vast.ai lists none.
    async fn cheapest_offer(&self, request: &LaunchRequest<'_>) -> Result<String, GpuCloudError> {
        let operation = format!("search offers for {}", request.instance_type);
        let query = json!({
            "gpu_name": {"eq": request.instance_type},
            "num_gpus": {"eq": std::iter::once(request).count()},
            "rentable": {"eq": true},
            "rented": {"eq": false},
            "verified": {"eq": true},
            "disk_space": {"gte": request.boot_disk_gb},
            "type": "ondemand",
            "order": [["dph_total", "asc"]],
        });
        let answer = self
            .call(
                &operation,
                reqwest::Method::POST,
                "/v0/bundles/",
                &[],
                Some(&query),
            )
            .await?;
        let offers = answer
            .pointer("/offers")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                GpuCloudError::response(VENDOR, &operation, format!("no offers list in {answer}"))
            })?;
        match offers.first() {
            Some(offer) => http::text(VENDOR, &operation, offer, "/id"),
            None => Err(GpuCloudError::capacity(
                VENDOR,
                request.instance_type,
                format!(
                    "Vast.ai lists no rentable verified on-demand offer of one {} with {} GB of disk",
                    request.instance_type, request.boot_disk_gb
                ),
            )),
        }
    }
}

/// The creation time Vast.ai states in epoch seconds.
fn started(instance: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    let seconds = instance.pointer("/start_date").and_then(Value::as_f64)?;
    chrono::DateTime::UNIX_EPOCH
        .checked_add_signed(chrono::TimeDelta::try_seconds(seconds.trunc() as i64)?)
}

/// One instance's lifecycle from `actual_status` (the container) read
/// against `intended_status` (what the renter asked for). Vast.ai documents
/// that an instance whose container is `exited`, `offline` or `unknown`
/// while it is meant to run never reaches `running`, so that is a failure.
fn state(operation: &str, instance: &Value) -> Result<MachineState, GpuCloudError> {
    let intended = http::optional_text(instance, "/intended_status");
    let stopped_on_purpose = intended.as_deref() == Some("stopped");
    match http::optional_text(instance, "/actual_status").as_deref() {
        None | Some("loading" | "created" | "creating") => Ok(MachineState::Provisioning),
        Some("running") => Ok(MachineState::Running),
        Some("stopped") => Ok(MachineState::Stopped),
        Some("exited") if stopped_on_purpose => Ok(MachineState::Stopped),
        Some("exited" | "offline" | "unknown") => Ok(MachineState::Failed),
        Some(other) => Err(GpuCloudError::response(
            VENDOR,
            operation,
            format!(
                "instance actual_status {other:?} (intended {intended:?}) is not one this adapter \
                 reads"
            ),
        )),
    }
}

fn machine(operation: &str, instance: &Value) -> Result<Machine, GpuCloudError> {
    // An instance without a label was not rented by Stado: its name is
    // empty, so the agent filter, which reads the fleet's name prefix, skips it.
    let name = match http::optional_text(instance, "/label") {
        Some(label) => label,
        None => String::new(),
    };
    Ok(Machine {
        native_id: http::text(VENDOR, operation, instance, "/id")?,
        name,
        instance_type: http::text(VENDOR, operation, instance, "/gpu_name")?,
        state: state(operation, instance)?,
        created_at: started(instance),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let offer = self.cheapest_offer(request).await?;
        let operation = format!("rent offer {offer} of {}", request.instance_type);
        let body = json!({
            "image": required_setting(VENDOR, "image")?,
            "label": request.name,
            "disk": request.boot_disk_gb,
            "runtype": "args",
            "args": ["bash", "-c", request.startup_script],
            "target_state": "running",
            "cancel_unavail": true,
        });
        let answer = self
            .call(
                &operation,
                reqwest::Method::PUT,
                &format!("/v0/asks/{offer}/"),
                &[],
                Some(&body),
            )
            .await?;
        if answer.pointer("/success").and_then(Value::as_bool) != Some(true) {
            return Err(GpuCloudError::response(
                VENDOR,
                &operation,
                format!("the rental was not confirmed: {answer}"),
            ));
        }
        let (_, launched_at) = http::launch_stamp();
        Ok(Machine {
            native_id: http::text(VENDOR, &operation, &answer, "/new_contract")?,
            name: request.name.to_string(),
            instance_type: request.instance_type.to_string(),
            state: MachineState::Provisioning,
            created_at: Some(launched_at),
        })
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        self.call(
            &format!("destroy instance {native_id}"),
            reqwest::Method::DELETE,
            &format!("/v0/instances/{native_id}/"),
            &[],
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
                &format!("/v0/instances/{native_id}/"),
                &[],
                None,
            )
            .await
        {
            Ok(answer) => match answer.pointer("/instances") {
                Some(instance @ Value::Object(_)) => machine(&operation, instance).map(Some),
                Some(Value::Null) | None => Ok(None),
                Some(other) => Err(GpuCloudError::response(
                    VENDOR,
                    &operation,
                    format!("instances is neither an instance nor null: {other}"),
                )),
            },
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list instances";
        let mut machines = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let query: Vec<(&str, &str)> = match after.as_deref() {
                Some(token) => vec![("after_token", token)],
                None => Vec::new(),
            };
            let page = self
                .call(
                    operation,
                    reqwest::Method::GET,
                    "/v1/instances/",
                    &query,
                    None,
                )
                .await?;
            let instances = page
                .pointer("/instances")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    GpuCloudError::response(
                        VENDOR,
                        operation,
                        format!("no instances list in {page}"),
                    )
                })?;
            for instance in instances {
                machines.push(machine(operation, instance)?);
            }
            match http::optional_text(&page, "/next_token") {
                Some(token) => after = Some(token),
                None => return Ok(machines),
            }
        }
    }
}
