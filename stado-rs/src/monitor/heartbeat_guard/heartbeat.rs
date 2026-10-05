//! Whether a running job is alive, judged by the promise its worker wrote
//! into the job document and by what the job wrote after that promise.
//!
//! The worker stamps `lease_expires_at` on every renewal: the time by which
//! it says it will renew again (its poll period plus its own measured
//! lateness and write time). While that time has not passed the job is
//! alive. Once it has, the job is still alive if something was written for
//! it afterwards — its `status/<job>/heartbeat` pulse (the renewal is
//! failing, the worker is not) or a checkpoint shard (the upload starved the
//! renewal). Nothing else, and no window of anyone's choosing, decides it.

use chrono::{DateTime, Utc};

use crate::models::Job;
use crate::queue::JobStorage;

use super::checkpoint::checkpoint_written_after;
use super::parse_iso_lenient;

/// What a running job's own record says about whether it is alive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobLiveness {
    /// Its worker promised a renewal by this time, and it has not passed.
    Promised(DateTime<Utc>),
    /// The promise passed at this time, but the job wrote a pulse or a
    /// checkpoint after it.
    WrittenAfter(DateTime<Utc>),
    /// The promise passed at this time and nothing was written since.
    Lapsed(DateTime<Utc>),
    /// The record carries no promise (claimed by a Stado from before
    /// promises): nothing says when it should have spoken, so it is kept.
    Unpromised,
}

impl JobLiveness {
    pub fn alive(&self) -> bool {
        !matches!(self, JobLiveness::Lapsed(_))
    }
}

/// The verdict for one running job at `now`.
pub async fn job_liveness(store: &JobStorage, job: &Job, now: DateTime<Utc>) -> JobLiveness {
    let Some(expires) = job
        .lease_expires_at
        .as_deref()
        .filter(|value| !value.is_empty())
        .and_then(parse_iso_lenient)
    else {
        return JobLiveness::Unpromised;
    };
    if expires > now {
        return JobLiveness::Promised(expires);
    }
    let pulse = store
        .backend()
        .updated_at(&format!("status/{}/heartbeat", job.job_id))
        .await;
    // A pulse the coordinator cannot read is not proof of death.
    let pulsed = match pulse {
        Ok(updated) => updated.is_some_and(|updated| updated > expires),
        Err(_) => true,
    };
    if pulsed || checkpoint_written_after(store, &job.command, expires).await {
        return JobLiveness::WrittenAfter(expires);
    }
    JobLiveness::Lapsed(expires)
}

/// Whether any of `jids` is a running job that is alive at `now`. A job no
/// longer in running/ is not alive here; a running/ read that fails is kept.
pub async fn any_job_alive(store: &JobStorage, jids: &[String], now: DateTime<Utc>) -> bool {
    for jid in jids.iter().filter(|jid| !jid.is_empty()) {
        let text = match store.download_text(&format!("running/{jid}.json")).await {
            Ok(text) => text,
            Err(_) => return true,
        };
        let Some(text) = text.filter(|text| !text.is_empty()) else {
            continue;
        };
        let Ok(job) = Job::from_json(&text) else {
            return true;
        };
        if job_liveness(store, &job, now).await.alive() {
            return true;
        }
    }
    false
}
