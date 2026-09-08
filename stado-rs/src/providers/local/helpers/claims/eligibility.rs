//! The rules that decide whether this agent may claim a queued job, and the
//! name of the first rule that says no.
//!
//! Every rule here is a hard constraint, not a preference: a pin, an
//! assignment, an exclusivity flag, this machine's platform, its accelerator,
//! and the submitter's cost cap.

use crate::config;
use crate::models::Job;
use crate::providers::local::helpers::gpu::capacity::compat_accel_types;
use crate::providers::local::helpers::{accel_hourly_rate, MODEL_RE};

/// Does `identity` name this consumer?
///
/// Three spellings of one host reach the queue and every one of them is
/// legitimate: the consumer id the agent publishes
/// (`local-Charless-Mac-mini.local`), the machine's own hostname
/// (`Charless-Mac-mini.local`), and the registry target name
/// (`charless-mac-mini`) that `stado submit --pinned-host` and the makespan
/// mirror write. Case is not load-bearing either: registry hostnames are
/// stored normalized while `consumer_id` carries the machine's verbatim
/// `gethostname()` casing.
///
/// One predicate for both `pinned_host` and `assigned_to`, because matching
/// one spelling and refusing the other is how 55 jobs pinned to the always-on
/// mac starved for seven days. `pinned_host` was tolerant, the makespan
/// matcher mirrored that same `pinned_host` into `assigned_to`
/// (`scheduler::makespan`), and the exact `assigned_to == consumer_id` test
/// then refused every job the pin had just admitted:
/// `eligibility_rejected=72, eligible_count=0` on a host reporting
/// `claiming: yes, blockers: none`.
fn names_this_consumer(identity: &str, consumer_id: &str, kind: &str) -> bool {
    if identity.is_empty() || consumer_id.is_empty() {
        return false;
    }
    let identity = identity.to_lowercase();
    let cid = consumer_id.to_lowercase();
    let kind_prefix = format!("{}-", kind.to_lowercase());
    let machine = cid.strip_prefix(&kind_prefix).unwrap_or(&cid);
    let bare = machine.strip_suffix(".local").unwrap_or(machine);
    identity == cid || identity == machine || identity == bare
}

/// Local-agent claim rules. Python `_job_eligible`.
///
/// NEW (0.4.100): if job.assigned_to was set by the centralized
/// coordinator matcher, only the agent whose consumer_id matches may
/// claim. Empty assigned_to means unassigned and any-eligible-agent may
/// claim (pre-0.4.100 back-compat).
///
/// NEW (0.4.131): job.exclusive=True is only eligible when no other job is
/// running. The caller passes its current job count so this filter runs at the
/// agent-side claim loop without needing the process table here.
///
/// NEW (0.4.379): job.pinned_host is an operator hard-pin from submit
/// time; only the named consumer may ever claim it. pinned_only=True
/// (registry target flag) reverses the default: this agent then claims
/// ONLY jobs explicitly routed to it (pinned_host or assigned_to), so a
/// shared workstation never picks up stray queue backlog. Pin matching is
/// case-insensitive: registry hostnames are stored normalized while
/// consumer_id carries the machine's verbatim gethostname() casing.
///
/// NEW (0.7.9): job.platform_os/job.architecture are a hard machine
/// constraint, not a preference. A native build job produces binaries for
/// exactly one platform, so a darwin-arm64 build claimed by a Linux agent
/// compiles the wrong artifact under the right name — or, more often, fails
/// halfway and reports it as the commit's fault. Either field empty is no
/// constraint, which is every job submitted before build fan-out existed.
///
/// Pin and assignment matching both go through [`names_this_consumer`], so the
/// two can never disagree about one host.
#[allow(clippy::too_many_arguments)]
pub fn job_eligible(
    job: &Job,
    gpu_type: &str,
    vram_gb: i64,
    kind: &str,
    consumer_id: &str,
    active_job_count: usize,
    pinned_only: bool,
) -> bool {
    eligibility_refusal(
        job,
        gpu_type,
        vram_gb,
        kind,
        consumer_id,
        active_job_count,
        pinned_only,
    )
    .is_none()
}

/// The name of the first rule that refuses this job, or `None` when the agent
/// may claim it.
///
/// [`job_eligible`] is this function's boolean, and the agent's capacity
/// broadcast carries the name. It counted `eligibility_rejected=72,
/// eligible_count=0` on the always-on mac for seven days without ever saying
/// WHICH of nine rules refused, so an operator holding that number could not
/// tell a wrong pin from a wrong platform from a wrong accelerator, and
/// reading it cost a day of inference over one host's process table. A count
/// with no reason is a measurement of the wrong thing.
///
/// Each name is the field the rule judges, so the answer points straight at
/// the job document or the registry declaration that has to change.
#[allow(clippy::too_many_arguments)]
pub fn eligibility_refusal(
    job: &Job,
    gpu_type: &str,
    vram_gb: i64,
    kind: &str,
    consumer_id: &str,
    active_job_count: usize,
    pinned_only: bool,
) -> Option<&'static str> {
    let pin_matches = names_this_consumer(&job.pinned_host, consumer_id, kind);
    if !job.pinned_host.is_empty() && !pin_matches {
        return Some("pinned_host names another consumer");
    }
    let assigned = job.assigned_to.as_str();
    let assigned_matches = names_this_consumer(assigned, consumer_id, kind);
    if !assigned.is_empty() && !consumer_id.is_empty() && !assigned_matches {
        return Some("assigned_to names another consumer");
    }
    if pinned_only && !pin_matches && !assigned_matches {
        return Some("host claims pinned work only and this job names nobody");
    }
    if job.exclusive && active_job_count > 0 {
        return Some("job is exclusive and another job is running");
    }
    // This host's own release platform, from the one place the rest of the
    // build learns it — the same word the registry records as the target's
    // `release_platform`. Alias-tolerant on the way in: the fleet spells this
    // machine "darwin"/"macos" and "arm64"/"aarch64", "amd64"/"x86_64".
    if !crate::targets::platform_accepts_job(
        crate::self_update::platform_triple_short().unwrap_or_default(),
        &job.platform_os,
        &job.architecture,
    ) {
        return Some("platform_os/architecture is not this machine");
    }
    if crate::capabilities::execution_adapter(kind)
        != Some(crate::capabilities::ExecutionAdapter::Local)
    {
        if let Some(caps) = MODEL_RE.captures(&job.command) {
            if config::is_local_only_model(caps[1].trim_matches(['\'', '"'])) {
                return Some("command names a local-only model on a non-local executor");
            }
        }
    }
    if job.pin_to_provider
        && !crate::capabilities::same_variant(
            crate::capabilities::RuntimeFacet::Execution,
            &job.provider,
            kind,
        )
    {
        return Some("pin_to_provider names another provider");
    }
    let job_accel = job.gpu_type.as_str();
    let matches = crate::capabilities::ProviderId::Local.matches(&job.provider)
        || job_accel.is_empty()
        || job_accel == gpu_type
        || (vram_gb > 0 && compat_accel_types(vram_gb).iter().any(|a| a == job_accel));
    if !matches {
        return Some("gpu_type is not this machine's accelerator");
    }
    let cap = job.max_cost_per_hour_usd;
    if cap > 0.0 && !job_accel.is_empty() {
        let rate = accel_hourly_rate(job_accel, job.preemptible);
        if rate > 0.0 && rate > cap {
            return Some("max_cost_per_hour_usd is below this accelerator's rate");
        }
    }
    None
}
