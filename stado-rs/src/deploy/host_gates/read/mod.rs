//! The reads: two ssh reads and one object read against a live host, in that
//! order, and nothing that writes.

use chrono::Utc;
use serde_json::Value;

use super::gates::HostGates;
use super::verdict::assemble;
use crate::deploy::{host_channel, host_disk, DeployError, Runner};
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

mod sources;

use sources::{publication, waiting_jobs};

pub(in crate::deploy) use sources::resolves_to;

/// Read every gate that decides whether `host` claims.
///
/// Two ssh reads and one object read, in that order. An unreachable host is an
/// error carrying the remote's own last line rather than a report full of
/// nulls: "this box is not answering" is a different answer from "this box is
/// answering and refuses to claim", and only the second one is what this
/// command was written to find.
///
/// The second ssh read — which store the host's agent is bound to — is the
/// only one that is allowed to fail quietly. By the time it runs, the disk and
/// the capacity reads have both succeeded, so there is a verdict worth
/// printing; a host that will not answer that one question gets
/// [`AGENT_STORE_UNREADABLE`] noted and keeps its verdict.
///
/// [`AGENT_STORE_UNREADABLE`]: super::AGENT_STORE_UNREADABLE
pub async fn read_host_gates(host: &str, runner: &Runner) -> Result<HostGates, DeployError> {
    let registry = host_channel::canonical_registry().await?;
    let target = host_channel::resolve_target(&registry, host)?.clone();

    let interval = target
        .disk_cleanup
        .as_ref()
        .map(|policy| policy.check_interval_seconds);
    // Only the sections this command reads. `assemble` below consumes
    // `usage`, `state` and `snapshots` and nothing else, while the full
    // script also walks `$HOME` with `du` for an `inventory` only
    // `space report` prints. Measured on `lukasz-macbook` on 2026-09-02, the
    // three fields take 0.8s and the full script had not finished in 180s,
    // so this command died on `remote_timeout` on the machine it was
    // running on and published no verdict at all — a gate condition nobody
    // can read is a gate condition that does not exist. The kept fields are
    // produced by the same section constants under either scope, so the
    // cheap read cannot answer differently from the expensive one.
    let output = host_channel::run_script(
        &target,
        &host_disk::remote_script_for(host_disk::DiskScope::GateInputs),
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the host did not report its disk state",
        )));
    }
    let reading = host_disk::parse_output(&output.stdout, interval);
    let agent_store = agent_store_backend(&target, runner).await;

    let store = JobStorage::new()
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    let publication = publication(&registry, &target, &store).await?;

    let mut gates = assemble(
        &target,
        &reading,
        publication.as_ref(),
        agent_store.as_deref(),
        Utc::now(),
    );
    gates.waiting_jobs = waiting_jobs(&registry, &target, &store, Utc::now()).await?;
    Ok(gates)
}

/// The `wc_storage_backend` this host's own installed binary resolves from the
/// config its services consume, or `None` when the host would not say.
///
/// Read with [`crate::cli::host::remote_config_output`] — the exact script
/// `stado host config-show` sends — and not a second remote script of this
/// module's own, for the same reason `host gates` and `space report` share one
/// `df`: two scripts reading one host's configuration would eventually read
/// two different configurations, under a different `HOME` or a different
/// `STADO_CONFIG`, and the whole finding here is which configuration that
/// host's services actually consume.
///
/// The field is `resolved.wc_storage_backend`: `config show` reports the file
/// it read and the values it resolved separately, and only the resolved half
/// is what the agent on that host actually binds its `JobStorage` to — a
/// `WC_STORAGE_BACKEND` exported by the unit beats the file, which is one of
/// the two ways the Mac mini got where it got.
///
/// Failure and a missing field collapse to the same `None` deliberately.
/// "The read did not happen" and "the read happened and said nothing about the
/// store" are the same finding for an operator: this command cannot tell them
/// where that agent publishes, and must say so rather than imply the store is
/// fine.
async fn agent_store_backend(target: &ComputeTarget, runner: &Runner) -> Option<String> {
    let stdout = crate::cli::host::remote_config_output(
        target,
        crate::cli::host::RemoteConfigAction::Show,
        runner,
    )
    .await
    .ok()?;
    serde_json::from_str::<Value>(&stdout)
        .ok()?
        .get("resolved")?
        .get("wc_storage_backend")
        .and_then(Value::as_str)
        .map(str::to_string)
}
