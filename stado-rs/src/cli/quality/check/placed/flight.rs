//! The placed checks in flight, read from the queue itself, and the
//! supersession of an older revision's check by a newer one.

use super::{bundle, BATCH};
use crate::cli::CmdError;
use crate::models::Job;

/// The product and revision a placed check's run id names, or nothing for a
/// run id this command did not write: `quality-check-<product>-<revision>-<stamp>-<pid>`.
fn checked(run_id: &str) -> Option<(&str, &str)> {
    let rest = run_id.strip_prefix(&format!("{BATCH}-"))?;
    let (rest, _pid) = rest.rsplit_once('-')?;
    let (rest, _stamp) = rest.rsplit_once('-')?;
    let (product, revision) = rest.rsplit_once('-')?;
    Some((product, revision))
}

/// The checks in flight, queued or running, with the product and revision
/// each names.
pub(super) async fn in_flight() -> Result<Vec<(Job, String, String)>, CmdError> {
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(|error| {
            CmdError::from(error).within("open the queue to read the checks in flight")
        })?;
    let mut found = Vec::new();
    for prefix in [crate::queue::runs::RUNNING, crate::queue::runs::QUEUE] {
        let jobs = crate::queue::listing::list_jobs(&store, prefix, crate::queue::Listing::Every)
            .await
            .map_err(|error| CmdError::from(error).within(format!("read queue prefix {prefix}")))?;
        for mut job in jobs.into_iter().filter(|job| job.batch_id == BATCH) {
            let Some((product, revision)) =
                checked(&job.run_id).map(|(p, r)| (p.to_string(), r.to_string()))
            else {
                continue;
            };
            job.state = prefix.to_string();
            found.push((job, product, revision));
        }
    }
    Ok(found)
}

/// Cancel every check of `product` in flight for another revision: the newer
/// check replaces it. A queued one never took its bundle, so the bundle goes
/// with it. A cancellation that fails is named and the new check stands.
pub(super) async fn supersede(
    store: &crate::queue::storage::JobStorage,
    flight: &[(Job, String, String)],
    product: &str,
    revision: &str,
) -> Vec<String> {
    let facade = match crate::machine::MachineFacade::new().await {
        Ok(facade) => facade,
        Err(error) => {
            eprintln!("stado quality check: the queue could not be opened, so older checks of {product} were not cancelled: {error}");
            return Vec::new();
        }
    };
    let mut superseded = Vec::new();
    for (job, _, older) in flight
        .iter()
        .filter(|(_, p, r)| p == product && r != revision)
    {
        match facade.cancel_job(&job.job_id).await {
            Ok(_) => {
                eprintln!(
                    "stado quality check: cancelled job {} ({} on {}), the check of {product} {older} this check of {revision} supersedes",
                    job.job_id, job.state, job.pinned_host
                );
                superseded.push(job.job_id.clone());
                if job.state == crate::queue::runs::QUEUE {
                    let key = bundle::key(product, older);
                    if let Err(error) = store.delete_blob(&key).await {
                        eprintln!("stado quality check: the superseded check's bundle {key} could not be removed: {error}");
                    }
                }
            }
            Err(error) => eprintln!(
                "stado quality check: job {} (the check of {product} {older}) could not be cancelled: {error}",
                job.job_id
            ),
        }
    }
    superseded
}
