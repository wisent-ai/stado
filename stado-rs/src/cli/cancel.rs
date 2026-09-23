//! `stado cancel JOB_ID [--terminate]` performs one durable, idempotent
//! cancellation transition. Every accepted cancellation writes the marker
//! consumed by agents/coordinators and moves the job to `cancelled/`; a
//! cancelled job is never deleted or mislabeled as failed.
//!
//! A recorded cloud instance is deleted before the state transition on every
//! path so cancellation cannot knowingly leave paid capacity behind.
//! `--terminate` additionally reports the instance lookup and fails loudly
//! when a running job has no recoverable instance record.

use crate::machine::{canonical_json, recorded_instance, utcnow};
use crate::models::job_state;
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

    // Stop any recorded paid capacity before publishing the terminal state.
    // The provider deletion contract is idempotent.
    let terminated = terminate_instance(&store, job_id).await?;
    if terminate {
        report(&terminated, job_id);
    }
    cancel_in_store(&store, job_id).await?;

    if terminate && matches!(terminated, Termination::NoRecord { expected: false }) {
        return Err(CmdError::click(format!(
            "--terminate found nothing to delete for {job_id}: it was running but neither \
             the job document nor provider lease records an instance. The durable cancellation \
             remains visible; inspect the provider inventory for orphaned capacity."
        )));
    }
    Ok(())
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

/// Publish one durable terminal transition. The marker is create-if-absent,
/// and terminal jobs make retries idempotent.
async fn cancel_in_store(store: &JobStorage, job_id: &str) -> Result<(), CmdError> {
    for prefix in ["cancelled", "completed", "uploaded", "failed"] {
        if let Some(job) = store.read_job(prefix, job_id).await? {
            println!("Job {job_id} is already terminal ({})", job.state);
            return Ok(());
        }
    }

    let marker = canonical_json(&serde_json::json!({
        "job_id": job_id,
        "requested_at": utcnow(),
    }));
    store
        .create_text_if_absent(&format!("cancellations/{job_id}.json"), &marker)
        .await?;

    if let Some(mut job) = store.read_job("queue", job_id).await? {
        job.state = job_state::CANCELLED.into();
        job.completed_at = Some(utcnow());
        job.error = Some("cancelled".into());
        store.write_job("cancelled", &job).await?;
        store.delete_job("queue", job_id).await?;
        if store.read_job("running", job_id).await?.is_none() {
            println!("Cancelled {job_id}");
            return Ok(());
        }
    }

    if let Some(mut job) = store.read_job("running", job_id).await? {
        job.state = job_state::CANCELLED.into();
        job.completed_at = Some(utcnow());
        job.error = Some("cancelled".into());
        job.instance_ref = None;
        store.write_job("cancelled", &job).await?;
        store.delete_job("running", job_id).await?;
        store.delete_job("failed", job_id).await?;
        println!("Cancelled {job_id}");
        return Ok(());
    }

    Err(CmdError::click(format!("Job {job_id} not found")))
}
