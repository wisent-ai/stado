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
        // `validate` admits only a request with a text command, so one without
        // it is a malformed request, refused before the record is marked as
        // enqueuing rather than submitted as an empty job.
        let Some(command) = request["command"].as_str().map(str::to_string) else {
            return Err(MachineError::new(
                "INTERNAL",
                format!("the request recorded at {record_path} names no command"),
            ));
        };
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
        // source_archive_path (Python dict comprehension). The validator
        // shaped every field read here; a field in another shape is a
        // request that bypassed it, refused by name rather than defaulted.
        let shaped = |name: &str| {
            MachineError::new(
                "INVALID_REQUEST",
                format!("the stored request's {name} is not in the shape the validator admits"),
            )
        };
        let text = |name: &str| -> Result<String, MachineError> {
            request[name]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| shaped(name))
        };
        let integer = |name: &str| -> Result<i64, MachineError> {
            request[name].as_i64().ok_or_else(|| shaped(name))
        };
        let flag = |name: &str| -> Result<bool, MachineError> {
            request[name].as_bool().ok_or_else(|| shaped(name))
        };
        let secret_env = request["secret_env"]
            .as_object()
            .ok_or_else(|| shaped("secret_env"))?
            .iter()
            .map(|(env_name, value)| {
                let spec = value.as_object().ok_or_else(|| shaped("secret_env"))?;
                let part = |key: &str| {
                    spec[key]
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| shaped(&format!("secret_env.{env_name}.{key}")))
                };
                Ok((
                    env_name.clone(),
                    JobSecretRef {
                        role: part("role")?,
                        field: part("field")?,
                    },
                ))
            })
            .collect::<Result<_, MachineError>>()?;
        let mut options = SubmitOptions {
            bucket: self.bucket.clone(),
            run_id: run_id.clone(),
            provider: text("provider")?,
            gpu_type: text("gpu_type")?,
            pinned_host: text("pinned_host")?,
            vram_gb: integer("vram_gb")?,
            max_cost_per_hour_usd: request["max_cost_per_hour_usd"]
                .as_f64()
                .ok_or_else(|| shaped("max_cost_per_hour_usd"))?,
            // A stored request that still names max_restarts came from the
            // marketplace, which sent it only to keep every restart its own.
            submitter_restarts: request.get("submitter_restarts").and_then(Value::as_bool)
                == Some(true)
                || request.get("max_restarts").is_some(),
            pin_to_provider: flag("pin_to_provider")?,
            priority: integer("priority")?,
            repo: text("repo")?,
            repo_ref: text("repo_ref")?,
            repo_workdir: text("repo_workdir")?,
            repo_extras: text("repo_extras")?,
            pre_command: text("pre_command")?,
            apt_packages: request["apt_packages"]
                .as_array()
                .ok_or_else(|| shaped("apt_packages"))?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| shaped("apt_packages"))
                })
                .collect::<Result<Vec<String>, MachineError>>()?,
            output_uri: text("output_uri")?,
            verify_command: text("verify_command")?,
            exclusive: flag("exclusive")?,
            terminates_agent: request.get("terminates_agent").and_then(Value::as_bool)
                == Some(true),
            boot_disk_gb: request.get("boot_disk_gb").and_then(Value::as_i64),
            runtime_seconds_estimate: request
                .get("runtime_seconds_estimate")
                .and_then(Value::as_f64),
            secret_env,
            resolved_input_artifacts: request["input_objects"]
                .as_object()
                .ok_or_else(|| shaped("input_objects"))?
                .clone(),
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
        let submitted = submit_batch(std::slice::from_ref(&command), &options).await;
        renew_machine_request_enqueue(&self.store, record_path, owner, "enqueue-complete").await?;
        let job = match submitted {
            Ok(mut jobs) => jobs.pop().ok_or_else(|| {
                MachineError::retryable("SUBMIT_FAILED", "durable submission returned no job")
            })?,
            Err(exc) => {
                // The request this facade claimed is released so another can
                // retry it at once. A record that cannot be read, or is not an
                // object, leaves it claimed, and that is said beside the
                // submission's own error instead of dropped.
                let versioned = match self.store.read_text_versioned(record_path).await {
                    Ok(Some(versioned)) => versioned,
                    Ok(None) => return Err(MachineError::retryable("SUBMIT_FAILED", exc.to_string())),
                    Err(unread) => {
                        return Err(MachineError::retryable(
                            "SUBMIT_FAILED",
                            format!("{exc}; the machine request stays claimed: it cannot be read back: {unread}"),
                        ))
                    }
                };
                let mut released = match serde_json::from_str::<Value>(&versioned.content) {
                    Ok(Value::Object(released)) => released,
                    Ok(other) => {
                        return Err(MachineError::retryable(
                            "SUBMIT_FAILED",
                            format!("{exc}; the machine request stays claimed: its record is not an object: {other}"),
                        ))
                    }
                    Err(unparsed) => {
                        return Err(MachineError::retryable(
                            "SUBMIT_FAILED",
                            format!("{exc}; the machine request stays claimed: its record is not JSON: {unparsed}"),
                        ))
                    }
                };
                if released.get("owner").and_then(Value::as_str) == Some(owner.as_str()) {
                    released.insert("state".into(), Value::from("released"));
                    released.remove("owner");
                    released.insert("last_error".into(), Value::from(exc.to_string()));
                    // A request left owned by this facade waits out its claim
                    // before another can retry it, so a release that did not
                    // land is part of the error.
                    if let Err(release) = self
                        .store
                        .compare_and_swap_text(
                            record_path,
                            &versioned.version,
                            &canonical_json(&Value::Object(released)),
                        )
                        .await
                    {
                        return Err(MachineError::retryable(
                            "SUBMIT_FAILED",
                            format!("{exc}; the machine request stays claimed: {release}"),
                        ));
                    }
                }
                return Err(MachineError::retryable("SUBMIT_FAILED", exc.to_string()));
            }
        };
        Ok(job)
    }
}
