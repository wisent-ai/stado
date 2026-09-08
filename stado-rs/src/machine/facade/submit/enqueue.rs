//! Turning a claimed reservation into a queued job: fence the record into
//! `enqueuing`, translate the request into submit options, and submit.

use serde_json::{Map, Value};

use crate::machine::contract::encoding::canonical_json;
use crate::machine::requests::leases::{
    renew_machine_request_claim, renew_machine_request_enqueue,
};
use crate::machine::{MachineError, MachineFacade};
use crate::models::{Job, JobSecretRef};
use crate::queue::submit::{submit_batch, SubmitOptions};

use super::{ClaimedReservation, SubmitRequestContext};

impl MachineFacade {
    /// Hand the reservation to the durable submitter, renewing the lease for
    /// as long as the submission takes.
    pub(super) async fn enqueue_machine_request(
        &self,
        ctx: &SubmitRequestContext,
        reserved: &ClaimedReservation,
    ) -> Result<Job, MachineError> {
        let SubmitRequestContext {
            request,
            request_id,
            record_path,
            run_id,
            owner,
            ..
        } = ctx;
        let ClaimedReservation {
            source_uri,
            source_sha,
            ..
        } = reserved;
        renew_machine_request_claim(&self.store, record_path, owner, "ready-to-enqueue").await?;

        let versioned = self
            .store
            .read_text_versioned(record_path)
            .await?
            .ok_or_else(|| {
                MachineError::retryable("REQUEST_IN_PROGRESS", "reservation disappeared")
            })?;
        let mut active: Map<String, Value> = serde_json::from_str::<Value>(&versioned.content)?
            .as_object()
            .cloned()
            .ok_or_else(|| MachineError::new("INTERNAL", "stored idempotency record is invalid"))?;
        if active.get("owner").and_then(Value::as_str) != Some(owner.as_str())
            || active.get("state").and_then(Value::as_str) != Some("claimed")
        {
            return Err(MachineError::retryable(
                "REQUEST_IN_PROGRESS",
                "matching request ownership changed before enqueue",
            ));
        }
        active.insert("state".into(), Value::from("enqueuing"));
        active.insert("phase".into(), Value::from("enqueue"));
        self.store
            .compare_and_swap_text(
                record_path,
                &versioned.version,
                &canonical_json(&Value::Object(active.clone())),
            )
            .await
            .map_err(|error| {
                MachineError::retryable(
                    "REQUEST_IN_PROGRESS",
                    format!("matching request ownership changed before enqueue: {error}"),
                )
            })?;

        // kwargs = normalized request minus client_request_id / command /
        // source_archive_path (Python dict comprehension).
        let str_field = |name: &str| request[name].as_str().unwrap_or_default().to_string();
        let mut options = SubmitOptions {
            bucket: self.bucket.clone(),
            run_id: run_id.clone(),
            provider: str_field("provider"),
            gpu_type: str_field("gpu_type"),
            pinned_host: str_field("pinned_host"),
            vram_gb: request["vram_gb"].as_i64().unwrap_or_default(),
            max_cost_per_hour_usd: request["max_cost_per_hour_usd"]
                .as_f64()
                .unwrap_or_default(),
            pin_to_provider: request["pin_to_provider"].as_bool().unwrap_or_default(),
            priority: request["priority"].as_i64().unwrap_or_default(),
            repo: str_field("repo"),
            repo_ref: str_field("repo_ref"),
            repo_workdir: str_field("repo_workdir"),
            repo_extras: str_field("repo_extras"),
            pre_command: str_field("pre_command"),
            apt_packages: request["apt_packages"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|i| i.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            output_uri: str_field("output_uri"),
            verify_command: str_field("verify_command"),
            exclusive: request["exclusive"].as_bool().unwrap_or_default(),
            secret_env: request["secret_env"]
                .as_object()
                .map(|items| {
                    items
                        .iter()
                        .map(|(env_name, value)| {
                            let spec = value.as_object().expect("validated secret_env object");
                            (
                                env_name.clone(),
                                JobSecretRef {
                                    item: spec["item"]
                                        .as_str()
                                        .expect("validated secret item")
                                        .to_string(),
                                    field: spec["field"]
                                        .as_str()
                                        .expect("validated secret field")
                                        .to_string(),
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            resolved_input_artifacts: request["input_objects"]
                .as_object()
                .cloned()
                .unwrap_or_default(),
            ..Default::default()
        };
        if !source_uri.is_empty() {
            // The trusted Stado agent materializes this object before spawning
            // the untrusted job. The child never receives storage credentials.
            options.resolved_input_artifacts.insert(
                "machine_source".into(),
                serde_json::json!({
                    "stado_uri": source_uri,
                    "relative_path": "machine-input.tar.gz",
                    "sha256": source_sha,
                }),
            );
            let work_component = format!("{request_id}-{source_sha}");
            let bootstrap = [
                "set -e".to_string(),
                format!(
                    "stado_machine_work=\"$HOME/.stado/work/machine/{work_component}\""
                ),
                "mkdir -p -- \"$HOME/.stado/work/machine\"".to_string(),
                "rm -rf -- \"$stado_machine_work\"".to_string(),
                "mkdir -p -- \"$stado_machine_work\"".to_string(),
                "tar --extract --gzip --file=\"$PWD/machine-input.tar.gz\" --directory=\"$stado_machine_work\" --no-same-owner --no-same-permissions".to_string(),
                "cd -- \"$stado_machine_work\"".to_string(),
            ]
            .join("\n");
            let caller_pre_command = &options.pre_command;
            options.pre_command = if caller_pre_command.is_empty() {
                bootstrap
            } else {
                format!("{bootstrap}\n{caller_pre_command}")
            };
            options.repo = String::new();
            options.repo_ref = String::new();
            options.repo_workdir = String::new();
            options.repo_extras = String::new();
        }
        let command = request["command"].as_str().unwrap_or_default().to_string();
        let submission = submit_batch(std::slice::from_ref(&command), &options);
        tokio::pin!(submission);
        let submitted = loop {
            tokio::select! {
                result = &mut submission => break result,
                _ = tokio::time::sleep(std::time::Duration::from_secs(5 * 60)) => {
                    renew_machine_request_enqueue(
                        &self.store,
                        record_path,
                        owner,
                        "enqueue",
                    )
                    .await?;
                }
            }
        };
        renew_machine_request_enqueue(&self.store, record_path, owner, "enqueue-complete").await?;
        let job = match submitted {
            Ok(mut jobs) => jobs.pop().ok_or_else(|| {
                MachineError::retryable("SUBMIT_FAILED", "durable submission returned no job")
            })?,
            Err(exc) => {
                if let Ok(Some(versioned)) = self.store.read_text_versioned(record_path).await {
                    if let Ok(Value::Object(mut released)) =
                        serde_json::from_str::<Value>(&versioned.content)
                    {
                        if released.get("owner").and_then(Value::as_str) == Some(owner.as_str()) {
                            released.insert("state".into(), Value::from("claimed"));
                            released.insert(
                                "lease_expires_at".into(),
                                Value::from(chrono::Utc::now().to_rfc3339()),
                            );
                            released.insert("last_error".into(), Value::from(exc.to_string()));
                            let _ = self
                                .store
                                .compare_and_swap_text(
                                    record_path,
                                    &versioned.version,
                                    &canonical_json(&Value::Object(released)),
                                )
                                .await;
                        }
                    }
                }
                return Err(MachineError::retryable("SUBMIT_FAILED", exc.to_string()));
            }
        };
        Ok(job)
    }
}
