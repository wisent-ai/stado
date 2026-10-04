//! Fencing: the queue side of it.
//!
//! Pausing a store is only half a fence — a paused store can still have jobs
//! in `running/`. So every fence here is pause-then-check, and a store with a
//! running job is an error that deliberately leaves both stores paused rather
//! than proceeding with a live writer.
//!
//! [`services`] is the other half: the declared writers, stopped before the
//! final copy and restarted only when explicitly activated.

pub(super) mod services;

use crate::cli::CmdError;
use crate::queue::control;
use crate::queue::copy::Endpoint;
use crate::queue::JobStorage;

pub(super) async fn endpoint_store(endpoint: &Endpoint) -> Result<JobStorage, CmdError> {
    let backend = endpoint.build().await?;
    Ok(JobStorage::with_backend_and_bucket(
        backend,
        endpoint.kind.clone(),
        endpoint.bucket.clone(),
    ))
}

pub(super) async fn drain_store(store: &JobStorage, description: &str) -> Result<(), CmdError> {
    let running = control::job_count(store, control::RUNNING_PREFIX).await?;
    if running == 0 {
        let queued = control::job_count(store, control::QUEUED_PREFIX).await?;
        println!("  {description} drained: running=0, queued={queued}, paused=true");
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "{description} still has {running} running job(s); both stores remain PAUSED. Run the \
         recovery again once they finish."
    )))
}
