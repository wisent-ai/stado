//! Declaring one schedule in the fleet store, shared by `schedule create`
//! and the delivery of declarations held on this host.

use crate::queue::{JobStorage, StorageError};
use crate::schedules::{self, read_schedule, Schedule};

use crate::cli::CmdError;

/// What declaring a schedule did in the store.
pub(super) enum Declared {
    Created(Schedule),
    Unchanged(Schedule),
    Converged(Schedule, Vec<&'static str>),
}

impl Declared {
    pub(super) fn schedule(&self) -> &Schedule {
        match self {
            Declared::Created(schedule)
            | Declared::Unchanged(schedule)
            | Declared::Converged(schedule, _) => schedule,
        }
    }

    /// What was done, as a clause after the schedule's id.
    pub(super) fn said(&self) -> String {
        match self {
            Declared::Created(_) => "written into the fleet store".to_string(),
            Declared::Unchanged(_) => "was already declared in the fleet store".to_string(),
            Declared::Converged(_, differs) => format!(
                "brought the stored schedule to its declaration: changed {}",
                differs.join(", ")
            ),
        }
    }
}

/// Declare `sched` in `store`: create it, or, under a caller-retained id,
/// leave the same declaration as it is, bring a different one to it with
/// `converge`, or refuse naming what differs. A store error is the outer
/// error, so a caller can tell a store that does not answer; a refusal is
/// the inner one.
pub(super) async fn declare(
    store: &JobStorage,
    sched: &Schedule,
    id_retained: bool,
    converge: bool,
) -> Result<Result<Declared, CmdError>, StorageError> {
    let sid = &sched.schedule_id;
    if id_retained {
        if let Some(existing) = read_schedule(store, sid).await? {
            // A caller-retained identity makes the declaration safe to repeat:
            // the same declaration again is already done, and a different one
            // is refused naming what differs, so an install can declare its
            // schedule on every run without ever overwriting another's.
            let differs = declaration_differences(&existing, sched);
            if converge && !existing.deleted && !differs.is_empty() {
                return bring_to(store, sched, differs).await;
            }
            if existing.deleted || !differs.is_empty() {
                return Ok(Err(CmdError::refused(format!(
                    "schedule {sid} already exists{}; change it with `stado schedule edit {sid}`, \
                     declare it again with --converge when this declaration owns the id, \
                     or remove it with `stado schedule rm {sid}`",
                    if existing.deleted {
                        " as a deleted record".to_string()
                    } else {
                        format!(" with a different {}", differs.join(", "))
                    }
                ))));
            }
            return Ok(Ok(Declared::Unchanged(existing)));
        }
    }
    schedules::write_schedule(store, sched).await?;
    Ok(Ok(Declared::Created(sched.clone())))
}

/// Bring the standing schedule under `wanted`'s id to `wanted`'s declaration
/// in place: every declared field is the caller's, while what the schedule
/// has done (created, fired, its last run and job, a leased occurrence) is
/// kept. Its next run is computed from now, as `schedule edit` computes it.
/// A record deleted meanwhile is refused, never raised again.
async fn bring_to(
    store: &JobStorage,
    wanted: &Schedule,
    differs: Vec<&'static str>,
) -> Result<Result<Declared, CmdError>, StorageError> {
    let sid = wanted.schedule_id.clone();
    let converged = schedules::edit_schedule(store, &sid, |standing| {
        standing.command = wanted.command.clone();
        standing.cron = wanted.cron.clone();
        standing.tz = wanted.tz.clone();
        standing.enabled = wanted.enabled;
        standing.provider = wanted.provider.clone();
        standing.pin_to_provider = wanted.pin_to_provider;
        standing.preemptible = wanted.preemptible;
        standing.max_cost_per_hour_usd = wanted.max_cost_per_hour_usd;
        standing.priority = wanted.priority;
        standing.gpu_type = wanted.gpu_type.clone();
        standing.vram_gb = wanted.vram_gb;
        standing.machine_type = wanted.machine_type.clone();
        standing.pinned_host = wanted.pinned_host.clone();
        standing.repo = wanted.repo.clone();
        standing.repo_ref = wanted.repo_ref.clone();
        standing.repo_workdir = wanted.repo_workdir.clone();
        standing.repo_extras = wanted.repo_extras.clone();
        standing.pre_command = wanted.pre_command.clone();
        standing.apt_packages = wanted.apt_packages.clone();
        standing.output_uri = wanted.output_uri.clone();
        standing.verify_command = wanted.verify_command.clone();
        standing.exclusive = wanted.exclusive;
        standing.boot_disk_gb = wanted.boot_disk_gb;
        standing.runtime_seconds_estimate = wanted.runtime_seconds_estimate;
        standing.secret_env = wanted.secret_env.clone();
        standing.overlap_policy = wanted.overlap_policy;
        standing.next_due_at = wanted.next_due_at.clone();
    })
    .await?;
    Ok(match converged {
        Some(converged) => Ok(Declared::Converged(converged, differs)),
        None => Err(CmdError::refused(format!(
            "schedule {sid} was removed while it was being brought to its declaration; \
             it is not declared again"
        ))),
    })
}

/// The declared fields in which `existing` differs from `wanted`: what the
/// schedule submits, when, where and with which secrets. Bookkeeping such as
/// its next run, its creator and its leases is not a declaration.
fn declaration_differences(existing: &Schedule, wanted: &Schedule) -> Vec<&'static str> {
    let mut differs = Vec::new();
    let mut compare = |name: &'static str, same: bool| {
        if !same {
            differs.push(name);
        }
    };
    compare("command", existing.command == wanted.command);
    compare("cron", existing.cron == wanted.cron);
    compare("timezone", existing.tz == wanted.tz);
    compare("enabled state", existing.enabled == wanted.enabled);
    compare("pinned host", existing.pinned_host == wanted.pinned_host);
    compare("provider", existing.provider == wanted.provider);
    compare(
        "secret environment",
        existing.secret_env == wanted.secret_env,
    );
    compare(
        "repository",
        existing.repo == wanted.repo && existing.repo_ref == wanted.repo_ref,
    );
    compare("pre-command", existing.pre_command == wanted.pre_command);
    compare(
        "overlap policy",
        existing.overlap_policy == wanted.overlap_policy,
    );
    differs
}
