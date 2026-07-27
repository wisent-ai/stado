//! `stado cancel JOB_ID` — port of the `cancel` command in `stado/cli.py`.
//!
//! Queue-path semantics are exactly cli.py's: a queued job is DELETED from
//! `queue/`; a running job with an instance_ref is moved `running/` ->
//! `failed/` with state=failed, error="cancelled" after the provider
//! terminates its instance. There is no `cancelled/`-prefix transition in
//! cli.py (v0.4.391) — the cancelled state exists in the model and in
//! runs' TERMINAL_PREFIXES but nothing in the cancel command writes it.

use crate::queue::submit::default_store;

use super::CmdError;

pub async fn run(job_id: &str) -> Result<(), CmdError> {
    if !super::api_key().is_empty() {
        // API mode (Python: DELETE {COMPUTE_API}/api/v1/instances/<id>).
        let response = reqwest::Client::new()
            .delete(format!("{}/api/v1/instances/{job_id}", crate::config::compute_api()))
            .header("X-API-Key", super::api_key())
            .send()
            .await?;
        if response.status().is_success() {
            println!("Cancelled {job_id}");
        } else {
            println!("Failed: {}", response.status().as_u16());
        }
        return Ok(());
    }

    let store = default_store(crate::config::bucket()).await?;
    if store.read_job("queue", job_id).await?.is_some() {
        store.delete_job("queue", job_id).await?;
        println!("Removed {job_id} from queue");
        return Ok(());
    }
    let job = store.read_job("running", job_id).await?;
    if let Some(mut job) = job {
        if let Some(instance_ref) = job.instance_ref.as_deref() {
            if !instance_ref.starts_with("local@") {
                let provider = crate::providers::get_provider(&job.provider)
                    .map_err(|exc| CmdError::click(exc.to_string()))?;
                provider
                    .delete_instance(instance_ref)
                    .await
                    .map_err(|exc| CmdError::click(format!("cancel failed: {exc}")))?;
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
