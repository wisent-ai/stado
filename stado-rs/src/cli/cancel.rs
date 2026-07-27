//! `stado cancel JOB_ID [--terminate]` — port of the `cancel` command in
//! `stado/cli.py`, plus the flag that closes the billing gap.
//!
//! Queue-path semantics are exactly cli.py's: a queued job is DELETED from
//! `queue/`; a running job with an instance_ref is moved `running/` ->
//! `failed/` with state=failed, error="cancelled" after the provider
//! terminates its instance. There is no `cancelled/`-prefix transition in
//! cli.py (v0.4.391) — the cancelled state exists in the model and in
//! runs' TERMINAL_PREFIXES but nothing in the cancel command writes it.
//!
//! `--terminate` has NO Python original. cli.py only ever reads the job
//! document's `instance_ref`, so the two cases where a VM exists but the
//! document does not name it — a dispatch that created the instance and
//! died before stamping the job, and a job already rewritten by a partial
//! cancel — leave a machine running that nothing reclaims. The flag
//! resolves the reference through [`recorded_instance`], which also reads
//! `provider-leases/<job_id>.json`, deletes the instance through
//! [`crate::providers::get_provider`], and refuses to exit zero when it
//! could not find anything to delete for a job that is supposed to have
//! one. Without the flag every call is byte-for-byte what it was.

use crate::machine::{recorded_instance, LOCAL_INSTANCE_PREFIX};
use crate::queue::submit::default_store;
use crate::queue::JobStorage;

use super::CmdError;

/// What `--terminate` did about the job's cloud instance.
enum Termination {
    /// A cloud instance was found and deleted.
    Deleted {
        instance_ref: String,
        source: String,
    },
    /// The reference names a local agent slot, not a cloud resource.
    Local { instance_ref: String },
    /// Neither the job document nor the provider lease names an instance.
    /// `expected` is true where that is the correct state — a job still in
    /// `queue/` has not reached a provider, and a terminal job's agent
    /// cleared the reference on the way out. It is false only for a job in
    /// `running/`, which by definition should be holding something.
    NoRecord { expected: bool },
}

pub async fn run(job_id: &str, terminate: bool) -> Result<(), CmdError> {
    let store = default_store(crate::config::bucket()).await?;

    // Terminate BEFORE the state transition. The transition rewrites the
    // blobs the reference lives in, and stopping the meter is the half that
    // must survive a crash in the middle.
    let terminated = if terminate {
        let outcome = terminate_instance(&store, job_id).await?;
        report(&outcome, job_id);
        Some(outcome)
    } else {
        None
    };

    cancel_in_store(&store, job_id, terminated.as_ref()).await?;

    match terminated {
        Some(Termination::NoRecord { expected: false }) => Err(CmdError::click(format!(
            "--terminate found nothing to delete for {job_id}: it is in running/ but neither \
             the job document nor provider-leases/{job_id}.json records an instance \
             reference. The job was cancelled; if a VM is still up, no queue state points \
             at it and it will keep billing — check the provider console."
        ))),
        _ => Ok(()),
    }
}

/// Resolve the job's provider instance and delete it.
async fn terminate_instance(store: &JobStorage, job_id: &str) -> Result<Termination, CmdError> {
    let recorded = recorded_instance(store, job_id).await.map_err(|exc| {
        CmdError::click(format!("cannot resolve the instance of {job_id}: {exc}"))
    })?;
    let Some(instance) = recorded else {
        let running = store.read_job("running", job_id).await?.is_some();
        return Ok(Termination::NoRecord { expected: !running });
    };
    if instance.local {
        return Ok(Termination::Local {
            instance_ref: instance.instance_ref,
        });
    }
    let provider = crate::providers::get_provider(&instance.provider)
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    provider
        .delete_instance(&instance.instance_ref)
        .await
        .map_err(|exc| {
            CmdError::click(format!(
                "deleting instance {} (recorded in {}) failed: {exc}",
                instance.instance_ref, instance.source
            ))
        })?;
    Ok(Termination::Deleted {
        instance_ref: instance.instance_ref,
        source: instance.source,
    })
}

/// Say what happened to the instance, including when nothing did.
fn report(outcome: &Termination, job_id: &str) {
    match outcome {
        Termination::Deleted {
            instance_ref,
            source,
        } => {
            println!("Deleted instance {instance_ref} (recorded in {source})");
        }
        Termination::Local { instance_ref } => {
            println!(
                "{instance_ref} is a local agent slot, not a cloud instance — \
                 nothing to delete"
            );
        }
        Termination::NoRecord { expected: true } => {
            println!("No cloud instance recorded for {job_id} — nothing to delete");
        }
        Termination::NoRecord { expected: false } => {
            println!(
                "NOTHING DELETED: {job_id} is running but no instance reference is recorded \
                 in its job document or in provider-leases/{job_id}.json"
            );
        }
    }
}

/// The cli.py cancel path. With `terminated` as `None` — every call without
/// `--terminate` — this is byte-for-byte the command as it shipped.
async fn cancel_in_store(
    store: &JobStorage,
    job_id: &str,
    terminated: Option<&Termination>,
) -> Result<(), CmdError> {
    if store.read_job("queue", job_id).await?.is_some() {
        store.delete_job("queue", job_id).await?;
        println!("Removed {job_id} from queue");
        return Ok(());
    }
    let job = store.read_job("running", job_id).await?;
    if let Some(mut job) = job {
        // cli.py performs the transition only inside `if instance_ref`, so
        // a running job whose reference only ever reached the provider
        // lease falls through to "not found" and stays in running/ for
        // good. Under --terminate the reference has already been resolved
        // and the instance dealt with, so the move no longer hangs off the
        // document field — and the command stops reporting "not found" for
        // a job it just terminated.
        if job.instance_ref.is_some() || terminated.is_some() {
            if let Some(instance_ref) = job.instance_ref.as_deref() {
                // --terminate already deleted it; the provider contract
                // makes a second delete a no-op, but re-issuing it risks
                // failing the cancel on a transient error after the VM is
                // already gone.
                let already_gone = matches!(terminated, Some(Termination::Deleted { .. }));
                if !already_gone && !instance_ref.starts_with(LOCAL_INSTANCE_PREFIX) {
                    let provider = crate::providers::get_provider(&job.provider)
                        .map_err(|exc| CmdError::click(exc.to_string()))?;
                    provider
                        .delete_instance(instance_ref)
                        .await
                        .map_err(|exc| CmdError::click(format!("cancel failed: {exc}")))?;
                }
            }
            job.state = "failed".into();
            job.error = Some("cancelled".into());
            job.instance_ref = None;
            store.move_job(&job, "running", "failed").await?;
            println!("Cancelled {job_id} (marked failed)");
            return Ok(());
        }
    }
    println!("Job {job_id} not found");
    Ok(())
}
