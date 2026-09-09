//! Independent diagnostic reads retain their own result, source and duration.
use chrono::Utc;
use serde_json::Value;

use super::gates::HostGates;
use super::verdict::assemble;
use super::HOST_DIAGNOSTIC_INCOMPLETE;
use crate::deploy::{host_channel, host_disk, DeployError, Runner};
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

mod observation;
mod sources;
pub(crate) use observation::{observe, READ_BUDGET};
pub use observation::{DiagnosticRead, ReadState};
pub(in crate::deploy) use sources::resolves_to;
use sources::{publication, waiting_jobs};

pub async fn read_host_gates(host: &str, runner: &Runner) -> Result<HostGates, DeployError> {
    let backend = crate::config::wc_storage_backend().to_string();
    let (registry, mut registry_read) = observe(
        "registry",
        format!("registry.json through {backend}"),
        async {
            crate::targets::fetch_registry_or_last_good()
                .await
                .map_err(|error| DeployError(error.to_string()))
        },
    )
    .await;
    let Some((registry, notice)) = registry else {
        let mut observations = vec![registry_read];
        for (operation, source) in [
            ("disk_usage", format!("{host}: df -Pk /")),
            ("host_state", format!("{host}: janitor state and snapshots")),
            (
                "agent_store",
                format!("{host}: effective storage configuration"),
            ),
            ("storage", backend.clone()),
            ("capacity", "capacity/".to_string()),
            ("queue", "queue/".to_string()),
        ] {
            observations.push(DiagnosticRead::skipped(
                operation,
                source,
                "registry read did not complete",
            ));
        }
        return Ok(HostGates {
            host: host.to_string(),
            fleet_store_backend: backend,
            observations,
            blockers: vec![HOST_DIAGNOSTIC_INCOMPLETE.to_string()],
            ..HostGates::default()
        });
    };
    if let Some(notice) = notice {
        crate::targets::report_registry_notice(&notice);
        registry_read.state = ReadState::Cached;
        registry_read.detail = Some(notice);
    }
    let target = host_channel::resolve_target(&registry, host)?;
    // Free space must survive a slow janitor, snapshot, or configuration read.
    // These scopes reuse the same producer sections as the normal disk report.
    let (usage, state, agent, storage) = tokio::join!(
        observe(
            "disk_usage",
            format!("{}: df -Pk /", target.name),
            disk_read(target, host_disk::DiskScope::UsageOnly, runner)
        ),
        observe(
            "host_state",
            format!("{}: janitor state and snapshots", target.name),
            disk_read(target, host_disk::DiskScope::StateOnly, runner)
        ),
        observe(
            "agent_store",
            format!("{}: effective storage configuration", target.name),
            agent_store_backend(target, runner)
        ),
        observe("storage", backend.clone(), async {
            JobStorage::new()
                .await
                .map_err(|error| DeployError(error.to_string()))
        }),
    );
    let state_observed = state.0.is_some();
    let mut reading = state.0.unwrap_or_default();
    reading.usage = usage.0.and_then(|reading| reading.usage);
    let mut observations = vec![registry_read, usage.1, state.1, agent.1, storage.1];
    let mut publication_value = None;
    let mut publication_observed = false;
    let mut waiting = Vec::new();
    if let Some(store) = storage.0 {
        let (published, queued) = tokio::join!(
            observe(
                "capacity",
                format!("{backend}:capacity/ for {}", target.name),
                publication(&registry, target, &store)
            ),
            observe(
                "queue",
                format!("{backend}:queue/ for {}", target.name),
                waiting_jobs(&registry, target, &store, Utc::now())
            ),
        );
        publication_observed = published.0.is_some();
        publication_value = published.0.flatten();
        let mut publication_read = published.1;
        if publication_observed && publication_value.is_none() {
            publication_read.state = ReadState::Absent;
            publication_read.detail =
                Some("no capacity publication names this registry target".to_string());
        }
        if let Some(jobs) = queued.0 {
            waiting = jobs;
        }
        observations.push(publication_read);
        observations.push(queued.1);
    } else {
        observations.push(DiagnosticRead::skipped(
            "capacity",
            format!("{backend}:capacity/"),
            "storage client did not open",
        ));
        observations.push(DiagnosticRead::skipped(
            "queue",
            format!("{backend}:queue/"),
            "storage client did not open",
        ));
    }
    let mut gates = assemble(
        target,
        &reading,
        publication_value.as_ref(),
        agent.0.as_deref(),
        Utc::now(),
        state_observed,
        publication_observed,
    );
    gates.waiting_jobs = waiting;
    gates.complete = observations.iter().all(DiagnosticRead::complete);
    gates.observations = observations;
    if !gates.complete {
        gates.claiming = false;
        gates.blockers.push(HOST_DIAGNOSTIC_INCOMPLETE.to_string());
    }
    Ok(gates)
}

async fn disk_read(
    target: &ComputeTarget,
    scope: host_disk::DiskScope,
    runner: &Runner,
) -> Result<host_disk::DiskReading, DeployError> {
    let output = host_channel::run_script_with_timeout(
        target,
        &host_disk::remote_script_for(scope),
        READ_BUDGET,
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "host read exited {}: {} {}",
            output.code, output.stderr, output.stdout
        )));
    }
    let interval = target
        .disk_cleanup
        .as_ref()
        .map(|policy| policy.check_interval_seconds);
    let reading = host_disk::parse_output(&output.stdout, interval);
    if scope == host_disk::DiskScope::UsageOnly && reading.usage.is_none() {
        return Err(DeployError(
            "the host command completed without a filesystem usage reading".to_string(),
        ));
    }
    Ok(reading)
}

async fn agent_store_backend(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<String, DeployError> {
    let stdout = crate::cli::host::remote_config_output(
        target,
        crate::cli::host::RemoteConfigAction::Show,
        runner,
    )
    .await
    .map_err(|error| DeployError(error.to_string()))?;
    let document: Value = serde_json::from_str(&stdout)
        .map_err(|error| DeployError(format!("host storage configuration is not JSON: {error}")))?;
    document
        .get("resolved")
        .and_then(|value| value.get("wc_storage_backend"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            DeployError("host configuration contains no resolved.wc_storage_backend".to_string())
        })
}
