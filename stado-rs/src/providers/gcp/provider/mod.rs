//! `GcpProvider` itself: the lazily resolved state, the `name@zone`
//! reference codec, the single-zone insert attempt and the agent-VM
//! listings. The `Provider` trait implementation — zone rotation, delete,
//! stop/start and the accelerator census — lives in the `lifecycle`
//! component; the pure insert-body builders in `body`.

mod body;
mod lifecycle;

use serde_json::Value;
use tokio::sync::OnceCell;

use crate::config;
use crate::providers::ProviderError;
use crate::queue::JobStorage;

use super::client::{GceClient, GceError};

pub use body::{instance_body, region_of_zone};

/// Python `_log`.
fn log(msg: &str) {
    eprintln!("[gcp] {msg}");
}

/// Python f-string rendering of a bool.
fn py_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}

/// Resolved-at-first-use provider state (see the module deviation note).
struct GcpState {
    client: GceClient,
    store: JobStorage,
}

/// Python `GCPProvider`.
pub struct GcpProvider {
    state: OnceCell<GcpState>,
}

impl GcpProvider {
    /// Python `GCPProvider()` — lazy in Rust (see the module docs).
    pub fn from_env() -> Self {
        GcpProvider {
            state: OnceCell::new(),
        }
    }

    /// Bind explicit client + storage (tests).
    async fn state(&self) -> Result<&GcpState, ProviderError> {
        self.state
            .get_or_try_init(|| async {
                let client = GceClient::new(config::project()).await?;
                let store = JobStorage::new().await?;
                Ok::<_, ProviderError>(GcpState { client, store })
            })
            .await
    }

    /// Python's `name@zone` ref builder.
    fn reference(name: &str, zone: &str) -> String {
        format!("{name}@{zone}")
    }

    /// Python `name, zone = instance_ref.split("@")` — a ref that does not
    /// split into exactly two parts is a ValueError.
    fn parse_ref(instance_ref: &str) -> Result<(&str, &str), ProviderError> {
        let parts: Vec<&str> = instance_ref.split('@').collect();
        if parts.len() != 2 {
            return Err(ProviderError::Value(format!(
                "invalid instance_ref (expected name@zone): {instance_ref}"
            )));
        }
        Ok((parts[0], parts[1]))
    }

    /// One zone insert attempt after the caller has confirmed any stale
    /// same-name instance is absent. The insert operation is then awaited.
    #[allow(clippy::too_many_arguments)]
    async fn attempt_zone(
        client: &GceClient,
        zone: &str,
        name: &str,
        machine_type: &str,
        accel_type: &str,
        boot_disk_gb: i64,
        image: &str,
        image_project: &str,
        startup_script: &str,
        preemptible: bool,
    ) -> Result<(), GceError> {
        let body = instance_body(
            name,
            zone,
            machine_type,
            accel_type,
            boot_disk_gb,
            image,
            image_project,
            startup_script,
            preemptible,
        );
        let insert_path = format!("/projects/{}/zones/{zone}/instances", client.project());
        let op = client
            .post(&insert_path, &body, &format!("insert {name}@{zone}"))
            .await?;
        let op_name = op.get("name").and_then(Value::as_str).ok_or_else(|| {
            GceError::Api(format!("GCE insert {name}@{zone} -> no operation name"))
        })?;
        client
            .wait_zone_operation(zone, op_name, &format!("insert {name}@{zone}"))
            .await
    }

    /// Python `list_running_instance_refs_with_age`: `(name@zone,
    /// age_in_seconds)` for every wisent-agent VM that is not genuinely
    /// TERMINATED. Used by the dead-agent reaper to cross-reference against
    /// live capacity broadcasts and to apply a boot grace period before
    /// culling.
    ///
    /// The filter intentionally narrows to `<prefix>-agent-*` (not just
    /// `<prefix>-*`): the broader pattern also matches unrelated service
    /// MIG instances in the same project (`wisent-mig-api-*`,
    /// `wisent-mig-inference-*`, `wisent-mig-images-*`) which never
    /// broadcast capacity, so the reaper would mass-delete them every tick.
    pub async fn list_running_instance_refs_with_age(
        &self,
    ) -> Result<Vec<(String, f64)>, ProviderError> {
        let state = self.state().await?;
        let filter = format!("name:{}-agent-*", config::INSTANCE_PREFIX);
        let instances = state.client.aggregated_instances(&filter).await?;
        let now = chrono::Utc::now();
        let mut out = Vec::new();
        for (zone, instance) in instances {
            // Only a genuinely TERMINATED (or absent) instance means the VM
            // is gone. The old `!= "RUNNING"` filter dropped VMs in
            // transient states GCE routinely passes through — PROVISIONING/
            // STAGING on boot and especially REPAIRING/STOPPING/SUSPENDING
            // during host maintenance & live migration (frequent for
            // long-running A100 VMs). A migrating VM briefly leaves this
            // list, the monitor's "cloud agent missing from fleet" path
            // then requeues a perfectly healthy job, and the VM rejoins
            // seconds later. Confirmed live: wisent-agent-a100-1778891822-0
            // was looping normally (agent log through 01:10:36) when the
            // coordinator declared it "VM gone" and requeued Qwen3 724084db
            // at 01:03:37 — a false positive. Treat any non-TERMINATED
            // status as present.
            let status = instance.get("status").and_then(Value::as_str).unwrap_or("");
            if status == "TERMINATED" {
                continue;
            }
            let created = instance
                .get("creationTimestamp")
                .and_then(Value::as_str)
                .unwrap_or("");
            let mut age = 0.0;
            if !created.is_empty() {
                // Python: datetime.fromisoformat(created.replace("Z",
                // "+00:00")); chrono parses RFC3339 "Z" directly.
                if let Ok(ct) = chrono::DateTime::parse_from_rfc3339(created) {
                    age = (now - ct.with_timezone(&chrono::Utc)).num_milliseconds() as f64 / 1000.0;
                }
            }
            let name = instance.get("name").and_then(Value::as_str).unwrap_or("");
            out.push((Self::reference(name, &zone), age));
        }
        Ok(out)
    }

    /// Python `list_running_instance_refs`: `name@zone` refs for all
    /// non-TERMINATED wisent-agent VMs.
    pub async fn list_running_instance_refs(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self
            .list_running_instance_refs_with_age()
            .await?
            .into_iter()
            .map(|(reference, _)| reference)
            .collect())
    }
}
