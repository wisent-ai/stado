//! `stado host gates HOST` — the one payload that answers "why is this host
//! claiming nothing".
//!
//! NO Python original. The incident it exists for: the Mac mini's data volume
//! sat at roughly 2 GiB free against a 55 GiB registry policy. Its queue agent
//! computes [`disk_cleanup::disk_pressure_unresolved`] every tick, publishes
//! that word in its capacity broadcast, and fails admission CLOSED while it is
//! true — zero free slots, no claim, deliberately
//! ([`crate::providers::local::agent`]). So the host claimed nothing for
//! hours, every release build queued behind it, and no command in this CLI
//! said any of it: `host disk` printed the free bytes and the policy but never
//! the admission verdict, `registry doctor` listed the host as broadcasting
//! normally, and the one fact that mattered — the agent had stopped claiming,
//! on purpose, for a reason it was republishing every tick — existed only
//! inside `capacity/<consumer>.json`, which nothing read.
//!
//! Four sources, joined here and re-derived nowhere:
//!
//! - the host's own capacity publication (`capacity/<consumer>.json`), whose
//!   `diag` words are reported VERBATIM. A blocker an operator reads here has
//!   to be greppable in the agent that published it, otherwise the CLI has
//!   invented a second vocabulary for the same condition;
//! - the registry target: its declared `slots`, and its
//!   [`crate::targets::DiskCleanupPolicy`] serialized as it stands;
//! - `df -Pk /` and the janitor's own state file, read with the exact script
//!   [`crate::deploy::host_disk`] sends, so `host gates` and `host disk` can
//!   never disagree about how much space this host has;
//! - the host's own effective `wc_storage_backend`, read with the exact script
//!   `stado host config-show` sends, and classified by
//!   [`crate::capabilities::storage_reach`]. The fourth source exists because
//!   of a second incident on the same machine: its agent unit was re-declared
//!   with a `STADO_CONFIG` that set the backend to `local`, so the agent
//!   published its capacity into an on-disk store on that one box and read a
//!   stale registry back out of it. Everything above kept reporting normally —
//!   the agent was running, it was publishing, its numbers were internally
//!   consistent — and the only true statement was that no host but that one
//!   could address a single object it wrote. Seventy-four jobs waited days.
//!
//! Read-only, and safe against a live production host: one ssh read of one
//! `df` and one `cat`, one ssh read of `stado config show`, plus one object
//! read. Nothing restarts, nothing cycles, nothing is deleted. The write
//! half — actually getting the space back — is
//! [`crate::deploy::host_reclaim`].

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

use super::host_channel;
use super::{host_disk, DeployError, Runner};
use crate::capabilities::{storage_reach, StorageReach};
use crate::providers::local::disk_cleanup;
use crate::queue::capacity::{self, Publication};
use crate::queue::JobStorage;
use crate::targets::{ComputeTarget, Registry};

/// The agent's own word for "I cannot prove there is room, so I claim
/// nothing": [`disk_cleanup::disk_pressure_unresolved`], published under this
/// exact key in the capacity broadcast's `diag`.
pub const DISK_PRESSURE_UNRESOLVED: &str = "disk_pressure_unresolved";

/// The agent publishes `disk_cleanup_policy_known: false` when it has no
/// validated low watermark at all. Reported as its negation because a blocker
/// list reads as a list of things that are wrong; the underlying key, and the
/// only place the value comes from, is still the agent's.
///
/// It never appears alone: an unknown threshold also makes
/// [`DISK_PRESSURE_UNRESOLVED`] true by that function's own truth table. It is
/// named separately because the two send an operator to different places — one
/// is a full disk, the other is a host that cannot read its own policy.
pub const DISK_CLEANUP_POLICY_UNKNOWN: &str = "disk_cleanup_policy_unknown";

/// `stado queue pause` is in effect; the agent publishes this per tick.
pub const QUEUE_PAUSED: &str = "queue_paused";

/// The registry pinned this host, so it claims only work addressed to it.
pub const PINNED_ONLY: &str = "pinned_only";

/// No capacity publication for this host exists at all. Not the agent's word,
/// because a silent agent has no words: the scheduler cannot see this host, so
/// it cannot be given anything.
pub const NO_CAPACITY_PUBLICATION: &str = "no_capacity_publication";

/// A publication older than [`capacity::CAPACITY_STALE_SECONDS`], which every
/// live-capacity reader in the fleet filters out. The row is reported anyway,
/// with its age, because "the agent said this an hour ago" and "nobody ever
/// said anything" are different findings.
pub const CAPACITY_PUBLICATION_STALE: &str = "capacity_publication_stale";

/// This host's queue agent is bound to a storage backend whose coordinates
/// carry only as far as the machine that wrote them
/// ([`StorageReach::Device`]), so everything it publishes — its capacity
/// broadcast, its claims, its view of the registry — lands in a store no other
/// host in the fleet can address.
///
/// Neither the agent's word nor the registry's: an agent writing into a
/// device-local store does not know it is alone, and this is the one blocker
/// in this list that the host cannot report about itself. The condition is
/// [`crate::capabilities::StorageReach`]'s, resolved from the host's own
/// effective `wc_storage_backend` read over the same channel `stado host
/// config-show` uses.
///
/// The incident: the Mac mini's agent unit was re-declared with a
/// `STADO_CONFIG` pointing at a config that set `wc_storage_backend` to
/// `local`, so the agent bound its `JobStorage` to `~/.stado/local-storage` on
/// that one machine. It kept ticking, kept reading a `registry.json` out of
/// that private store — a stale 20 GiB watermark against a canonical 15 —
/// kept computing [`DISK_PRESSURE_UNRESOLVED`] against it, and kept publishing
/// capacity nothing in the fleet could ever read. Seventy-four jobs, fifty-five
/// of them pinned to that host, sat in the fleet queue for days. Every surface
/// in this CLI reported the host in-sync, and this command — the one command
/// written to answer "why is this host claiming nothing" — could say only
/// [`CAPACITY_PUBLICATION_STALE`], which was true and was a symptom.
///
/// Reported BEFORE [`CAPACITY_PUBLICATION_STALE`] for exactly that reason: a
/// host addressing a private store has no way to publish anything the control
/// plane will see, so its publication is stale by construction and the
/// staleness is downstream of this.
pub const AGENT_STORE_DEVICE_ONLY: &str = "agent_store_device_only";

/// This host answered with a storage backend this build has no adapter for, so
/// how far its agent's writes carry is not a thing this command can decide.
///
/// A blocker and not a note, and deliberately: the two cases where a host's
/// store cannot be shown to be the fleet's are "it demonstrably is not"
/// ([`AGENT_STORE_DEVICE_ONLY`]) and "this control plane cannot tell", and the
/// second one is how the first one gets missed for a week. It usually means
/// the host is running a newer or older Stado than the machine asking.
pub const AGENT_STORE_UNKNOWN: &str = "agent_store_unknown";

/// The host did not answer with a storage backend at all — the remote `config
/// show` failed, or its output carried no `wc_storage_backend`.
///
/// A NOTE and never a blocker: the disk read and the capacity read both
/// succeeded to get this far, so the verdict this command reports is still the
/// verdict, and a store read that could not be taken is a gap in the
/// diagnosis rather than a reason the host claims nothing. It is reported
/// because the alternative is a report that silently omits the store line and
/// reads as "the store is fine".
pub const AGENT_STORE_UNREADABLE: &str = "agent_store_unreadable";

/// Local APFS snapshots are holding space while this host cannot prove it has
/// room, and no command in this product will take them.
///
/// A NOTE and never a blocker: snapshots do not stop the agent claiming, disk
/// pressure does, and a host claiming normally with snapshots on it is a
/// healthy host. It is reported because of what an operator does next — they
/// run `stado host reclaim`, watch it free what it can, see the host still
/// short of its watermark, and have to be told where the rest of the space is
/// rather than left to conclude the numbers are lying. macOS publishes no size
/// for a snapshot ([`host_disk::LocalSnapshots`]), so this says how many there
/// are and never how large they are.
pub const LOCAL_SNAPSHOTS_UNRECLAIMABLE: &str = "local_snapshots_unreclaimable";

/// This host's janitor has not completed a pass within
/// [`STALL_INTERVALS`] times its own declared `check_interval_seconds`.
///
/// A BLOCKER while the disk is also under pressure, and a note otherwise, and
/// its own condition rather than a shade of [`DISK_PRESSURE_UNRESOLVED`]:
/// those two are the disk being full and the mechanism that empties it being
/// dead, they fail at different times, and the second one is the one nothing
/// in this product could see. On `lukasz-macbook` the janitor logged 12,197
/// passes between 2026-08-18 and 2026-09-02 and deleted nothing in any of
/// them — 8,539 never resolved a policy and 2,030 never got the run lock — so
/// `last_success_at` stayed null for fifteen days while every gate in the
/// fleet read green. The host then crossed its low watermark, releases stopped
/// fleet-wide, and the space came back by hand at one in the morning.
///
/// Two things this deliberately is not. It is not a pass that was PREVENTED:
/// a workload holds the run lock in shared mode for its whole duration and
/// every pass meanwhile answers `lock_busy`, which is the modelled answer and
/// not a fault, so a janitor being turned away is measured by
/// `last_prevented_at` and never accumulates here. And it does not refuse work
/// on a host that still has its headroom: below the watermark a stalled
/// janitor must block, because nothing is bringing the space back and
/// admitting a job is how the incident above ended; above it, refusing work
/// creates no space and only removes capacity.
///
/// Hosts that declare `mode: "off"` are exempt — a janitor nobody armed is not
/// a janitor that stalled — and so is a host with no declared interval to be
/// late against, which [`DISK_CLEANUP_POLICY_UNKNOWN`] already reports.
pub const DISK_CLEANUP_STALLED: &str = "disk_cleanup_stalled";

/// This host's janitor is being refused the run lock and has not completed a
/// pass within [`STALL_INTERVALS`] of its own declared interval: the lock is
/// not being taken turns with, it is HELD.
///
/// Its own word and not a shade of [`DISK_CLEANUP_STALLED`], because the two
/// send an operator to opposite places. A stalled janitor is a janitor that
/// ran and got nowhere — read its report, its policy, its errors. A held lock
/// is a janitor that never started, and the only thing worth looking at is the
/// process on the other end of `~/.cache/wisent-compute/disk-cleanup.lock`,
/// which `stado host disk` names in `cleanup_lock.holders`.
///
/// On 2026-09-03 charless-mac-mini reported the stalled word with 18.4 GiB
/// free against a 15 GiB watermark while its own agent (pid 79473) held the
/// lock, and lukasz-macbook reported it with 118.7 GiB free against 100. Both
/// pointed at a disk that was fine. The mechanism —
/// [`crate::providers::local::slots::release_hold_for_exited_workload`] — is
/// fixed, and this word exists so the next hold that outlives its workload is
/// read as a lock and not as a full disk.
///
/// Blocks on the same rule as [`DISK_CLEANUP_STALLED`] and for the same
/// reason: under pressure a janitor that cannot run must refuse work, and
/// above the watermark refusing work creates no space. It is a note there.
pub const DISK_CLEANUP_LOCK_HELD: &str = "disk_cleanup_lock_held";

/// How many of its own check intervals a janitor may miss before
/// [`DISK_CLEANUP_STALLED`] fires.
///
/// Four, because one missed pass is a lock this host lost to its own agent
/// tick and two is a registry read that timed out twice — both routine, both
/// self-correcting, and a gate that fires on them is a gate that gets muted.
/// Four consecutive misses is no longer weather: at the hourly interval this
/// fleet declares it is a janitor that has been silent for half a working
/// day, and at the ten-second agent tick it is forty seconds.
const STALL_INTERVALS: i64 = 4;

/// Everything one host answered about whether it can claim.
#[derive(Debug, Clone, PartialEq)]
pub struct HostGates {
    /// The registry target name, not the operator's spelling of it.
    pub host: String,
    /// `blockers.is_empty()`. Busy slots are NOT a blocker: a host running
    /// work claims nothing more and is perfectly healthy, and calling that
    /// blocked would make this command cry wolf on every loaded box.
    pub claiming: bool,
    pub blockers: Vec<String>,
    pub disk_pressure_unresolved: bool,
    /// [`DISK_CLEANUP_STALLED`]: the janitor has not completed a pass within
    /// `STALL_INTERVALS` of its own declared interval. Carried as a field and
    /// not only as a blocker string because the release verdict embeds it
    /// beside `disk_pressure_unresolved` ([`gates_section`]), and an operator
    /// reading "free 45 GiB against a 100 GiB watermark" has to be able to see
    /// in the same object whether anything is still trying to fix it.
    pub disk_cleanup_stalled: bool,
    /// [`DISK_CLEANUP_LOCK_HELD`]: the janitor is being refused the run lock
    /// AND has completed nothing inside the stall window. Carried beside
    /// `disk_cleanup_stalled` and never merged into it: the two are mutually
    /// exclusive by construction and name different remedies.
    pub disk_cleanup_lock_held: bool,
    /// Seconds since a pass was last PREVENTED from taking the run lock, or
    /// `None` when none was. The number behind `disk_cleanup_lock_held`, and
    /// the one that distinguishes "a workload is holding it right now" from "a
    /// hold has outlived its workload".
    pub cleanup_prevented_age_seconds: Option<i64>,
    /// Seconds since the janitor last completed a pass, or `None` when it has
    /// never recorded one. `None` with a declared interval is the fifteen-day
    /// case, and is not the same finding as "it succeeded a long time ago".
    pub cleanup_success_age_seconds: Option<i64>,
    /// `df -Pk /` available blocks as GiB, one decimal.
    pub free_gb: Option<f64>,
    /// The threshold admission is actually gated on: the janitor's own
    /// validated watermark first, the registry declaration second — the same
    /// order the agent resolves it in.
    pub low_watermark_gb: Option<i64>,
    pub target_free_gb: Option<i64>,
    pub policy_mode: Option<String>,
    pub published_at: Option<String>,
    pub age_seconds: Option<i64>,
    /// Free slots summed over accelerator types, or `None` when this host has
    /// published nothing.
    pub free_slots: Option<i64>,
    pub slots_declared: i64,
    /// The storage backend this host's own installed binary resolves from the
    /// config its services consume, or `None` when the host would not answer
    /// with one ([`AGENT_STORE_UNREADABLE`]). Reported beside
    /// `fleet_store_backend` and never compared away to a boolean: an
    /// operator has to be able to read "that host writes to `local`, the fleet
    /// reads `stado`" off one screen and go fix the unit that set it.
    pub agent_store_backend: Option<String>,
    /// The storage backend THIS control plane reads
    /// ([`crate::config::wc_storage_backend`]) — the other half of the
    /// sentence, because a backend name alone says nothing about whether the
    /// two ends agree.
    pub fleet_store_backend: String,
    /// Findings that are true and are NOT reasons this host claims nothing, so
    /// they can never change `claiming` or the exit status: an operator needs
    /// them to act, and a note that could fail a script would be suppressed
    /// within the week.
    pub notes: Vec<String>,
    /// How many local APFS snapshots the host is holding, or `None` where the
    /// host could not be asked (every Linux host).
    pub local_snapshots: Option<usize>,
    /// Queued jobs pinned to this host, oldest first. This is the gate's
    /// consequence made visible: a host that claims nothing while work is
    /// pinned to it is starving that exact list, and "blocked" without the
    /// starved work named is a verdict nobody can size.
    pub waiting_jobs: Vec<WaitingJob>,
}

/// One queued job a non-claiming host is starving.
#[derive(Debug, Clone, PartialEq)]
pub struct WaitingJob {
    pub job_id: String,
    pub age_seconds: Option<i64>,
}

/// The word this command reports when a host declares a queue agent and its
/// newest health beacon does not report that unit running.
///
/// Not the agent's word either — a unit that was never loaded publishes
/// nothing — but the registry's and the beacon's, joined. It exists because
/// [`NO_CAPACITY_PUBLICATION`] states only that a host is silent, and the
/// commonest cause of that silence in this fleet is a declaration naming a
/// unit no launchd or systemd on that host is running.
pub const AGENT_DECLARED_NOT_LOADED: &str = "agent_declared_not_loaded";

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
    // `host disk` prints. Measured on `lukasz-macbook` on 2026-09-02, the
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
/// module's own, for the same reason `host gates` and `host disk` share one
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
    let stdout = crate::cli::host::remote_config_output(target, None, runner)
        .await
        .ok()?;
    serde_json::from_str::<Value>(&stdout)
        .ok()?
        .get("resolved")?
        .get("wc_storage_backend")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Join the four sources into the verdict.
///
/// Split out from the reads, and public, so the truth table is exercisable
/// without a host, a registry or a store — which is the only way a gate that
/// decides whether a platform has any builder at all can be held to its
/// truth table rather than to whatever a live fleet happened to be doing.
///
/// `agent_store` is the host's own effective `wc_storage_backend`, or `None`
/// when the host would not answer with one.
pub fn assemble(
    target: &ComputeTarget,
    reading: &host_disk::DiskReading,
    publication: Option<&Publication>,
    agent_store: Option<&str>,
    now: DateTime<Utc>,
) -> HostGates {
    let policy = target.disk_cleanup.as_ref();
    let free_kb = reading
        .usage
        .as_ref()
        .and_then(|usage| usage.available_kb.parse::<f64>().ok());
    let free_gb = free_kb.map(host_disk::gib_from_blocks);
    // The registry's declared watermark first, and the janitor's state file
    // only where the registry declares no policy at all.
    //
    // This was the other way round, on the reasoning that the state file holds
    // the number the agent actually gated on and survives a registry the host
    // cannot read. Both halves are true and it still reported a number that
    // could not be acted on. That file is written by every cleanup pass, and on
    // an always-on host several processes make them: the queue agent every ten
    // seconds, a `disk-cleanup --watch` unit on its own timer, and any of them
    // may be a long-running process still holding a configuration that resolves
    // a superseded policy. On charless-mac-mini that produced `low watermark
    // 20 GiB, target 18 GiB` — a floor above its own ceiling, from a stale
    // 20/25 policy — alternating with the canonical 15/18 between one reading
    // and the next, while the registry said 15 throughout.
    //
    // So the declaration wins. It is what the fleet decided, this command has
    // just read it, and a watermark the operator cannot reconcile with the
    // policy document is worse than no watermark at all.
    let low_watermark_gb = policy.map(|policy| policy.low_free_gb).or_else(|| {
        reading
            .state
            .low_bytes
            .map(|bytes| bytes / disk_cleanup::GIB)
    });

    let payload = publication.map(|row| &row.payload);
    let published_at = payload
        .and_then(|payload| payload.get("published_at"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let age_seconds = publication
        .and_then(|row| row.stamp)
        .map(|stamp| (now - stamp).num_seconds());
    let stale = age_seconds.is_some_and(|age| age > capacity::CAPACITY_STALE_SECONDS as i64);

    // The published verdict while the row is live — that IS the decision the
    // agent is making right now. Once the row is stale or absent the agent is
    // no longer talking, so the same function it uses is applied to the
    // numbers this command just measured itself.
    let published_pressure = diag_flag(payload, DISK_PRESSURE_UNRESOLVED);
    let disk_pressure_unresolved = match published_pressure {
        Some(published) if !stale => published,
        _ => disk_cleanup::disk_pressure_unresolved(
            low_watermark_gb.map(|gb| gb * disk_cleanup::GIB),
            free_kb.map(|blocks| (blocks * 1024.0) as i64),
        ),
    };

    // How late the janitor is against the interval IT declares, measured from
    // the last pass that actually completed. `last_success_at` and not
    // `last_pass_at`: the incident this exists for logged a pass every sixty
    // seconds for fifteen days, so "it ran recently" was true throughout and
    // meant nothing.
    let cleanup_success_age_seconds = reading
        .state
        .last_success_at
        .as_deref()
        .and_then(|stamp| DateTime::parse_from_rfc3339(&stamp.replace('Z', "+00:00")).ok())
        .map(|stamp| (now - stamp.with_timezone(&Utc)).num_seconds());
    // Armed only where the host declares a janitor that is supposed to run.
    // `mode: "off"` is a deliberate choice and never late, and a host with no
    // declared interval has nothing to be late against — that is
    // `disk_cleanup_policy_unknown`, which is already a blocker of its own.
    let stall_after_seconds = policy
        .filter(|policy| policy.mode != "off")
        .map(|policy| policy.check_interval_seconds * STALL_INTERVALS);
    // How long the janitor has been PREVENTED rather than silent. A workload
    // holds the run lock in shared mode for its whole duration, by design, and
    // every pass that starts meanwhile answers `lock_busy` — the modelled
    // answer, not a fault.
    let cleanup_prevented_age_seconds = reading
        .state
        .last_prevented_at
        .as_deref()
        .and_then(|stamp| DateTime::parse_from_rfc3339(&stamp.replace('Z', "+00:00")).ok())
        .map(|stamp| (now - stamp.with_timezone(&Utc)).num_seconds());
    // A pass prevented within the same window the stall is measured over is a
    // janitor that is still running and still being turned away, so the age of
    // its last success says nothing about its health. Only silence does.
    //
    // This is the whole of the 2026-09-03 false blocker: charless-mac-mini ran
    // one job for 42 minutes, the in-process janitor polled every ten seconds
    // throughout, and because a prevented pass recorded nothing the success age
    // reached 2311s against a 1200s limit and `claiming` went off — on a host
    // with 17.3 GiB free against a 15 GiB watermark and
    // `disk_pressure_unresolved: false`. The host was refusing new work because
    // it was doing work.
    let cleanup_prevented = match (stall_after_seconds, cleanup_prevented_age_seconds) {
        (Some(limit), Some(age)) => age <= limit,
        _ => false,
    };
    // Being turned away is healthy for as long as somebody is taking turns.
    // Being turned away while nothing has got through for the whole window the
    // stall is measured over is not being turned away — it is a lock that is
    // held, and it has a different remedy from every other condition here:
    // find the holder (`host disk`'s `cleanup_lock.holders` names the pid) and
    // deal with THAT process. See [`DISK_CLEANUP_LOCK_HELD`].
    let disk_cleanup_lock_held = cleanup_prevented
        && match (stall_after_seconds, cleanup_success_age_seconds) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(limit), Some(age)) => age > limit,
        };
    let disk_cleanup_stalled = !cleanup_prevented
        && match (stall_after_seconds, cleanup_success_age_seconds) {
            (None, _) => false,
            // Declared, armed, and no completed pass on record at all. Reported
            // rather than excused: a janitor that has never finished a pass is
            // the fifteen-day case exactly, and the state file being absent or
            // fresh says nothing about whether the thing ever worked.
            (Some(_), None) => true,
            (Some(limit), Some(age)) => age > limit,
        };

    let mut blockers: Vec<String> = Vec::new();
    // First in the vector, ahead of the staleness it causes: an agent bound to
    // a device-local store cannot publish anything this control plane will
    // ever read, so its publication is missing or stale BY CONSTRUCTION, and
    // an operator reading `capacity_publication_stale` first goes looking at
    // the agent's uptime instead of at the config its unit exports.
    match agent_store.map(storage_reach) {
        Some(Some(StorageReach::Fleet)) | None => {}
        Some(Some(StorageReach::Device)) => blockers.push(AGENT_STORE_DEVICE_ONLY.to_string()),
        Some(None) => blockers.push(AGENT_STORE_UNKNOWN.to_string()),
    }
    if publication.is_none() {
        blockers.push(NO_CAPACITY_PUBLICATION.to_string());
    } else if stale {
        blockers.push(CAPACITY_PUBLICATION_STALE.to_string());
    }
    if disk_pressure_unresolved {
        blockers.push(DISK_PRESSURE_UNRESOLVED.to_string());
    }
    // Directly after the pressure it explains: an operator who reads
    // "free 45 GiB, watermark 100 GiB" needs the next line to say whether
    // anything is still trying, and for fifteen days there was no such line.
    //
    // It blocks only while the disk is also under pressure, and that is the
    // case where a stalled janitor genuinely must refuse work: the host is
    // already below the watermark, nothing is bringing it back, and admitting
    // a job onto an unmanaged disk is how the fifteen-day incident ended. Above
    // the watermark it is a NOTE. Refusing work on a host with headroom does
    // not create a single byte of space; it only removes capacity from the
    // fleet, and it removed the always-on Mac from the fleet on 2026-09-03 over
    // a janitor that was healthy. The condition stays visible either way —
    // `disk_cleanup_stalled` is carried as a field and embedded in the release
    // verdict, so nothing that could see this before has stopped seeing it.
    if disk_cleanup_stalled && disk_pressure_unresolved {
        blockers.push(DISK_CLEANUP_STALLED.to_string());
    }
    if disk_cleanup_lock_held && disk_pressure_unresolved {
        blockers.push(DISK_CLEANUP_LOCK_HELD.to_string());
    }
    if diag_flag(payload, "disk_cleanup_policy_known") == Some(false) || low_watermark_gb.is_none()
    {
        blockers.push(DISK_CLEANUP_POLICY_UNKNOWN.to_string());
    }
    if diag_flag(payload, QUEUE_PAUSED) == Some(true) {
        blockers.push(QUEUE_PAUSED.to_string());
    }

    // The note fires only while the disk is the reason this host claims
    // nothing: snapshots on a healthy box are a backup policy, not a finding,
    // and a command that reports them every time is a command whose output
    // stops being read.
    let local_snapshots = reading
        .snapshots
        .supported
        .then_some(reading.snapshots.names.len());
    let mut notes: Vec<String> = Vec::new();
    if diag_flag(payload, PINNED_ONLY) == Some(true) || target.pinned_only {
        notes.push(PINNED_ONLY.to_string());
    }
    if disk_pressure_unresolved && local_snapshots.is_some_and(|count| count > 0) {
        notes.push(LOCAL_SNAPSHOTS_UNRECLAIMABLE.to_string());
    }
    // A janitor that is late on a host that still has its headroom. Not a
    // blocker (see the pressure gate above) and not silence either: an
    // operator has to be told that the mechanism which maintains this host's
    // free space is not running, before the day it matters.
    if disk_cleanup_stalled && !disk_pressure_unresolved {
        notes.push(DISK_CLEANUP_STALLED.to_string());
    }
    // The same finding for a lock that is held rather than a janitor that is
    // silent, and a note for the same reason: a host with headroom that cannot
    // clean is a host to go fix, not a host to close.
    if disk_cleanup_lock_held && !disk_pressure_unresolved {
        notes.push(DISK_CLEANUP_LOCK_HELD.to_string());
    }
    if agent_store.is_none() {
        notes.push(AGENT_STORE_UNREADABLE.to_string());
    }

    HostGates {
        host: target.name.clone(),
        claiming: blockers.is_empty(),
        blockers,
        disk_pressure_unresolved,
        disk_cleanup_stalled,
        disk_cleanup_lock_held,
        cleanup_success_age_seconds,
        cleanup_prevented_age_seconds,
        free_gb,
        low_watermark_gb,
        target_free_gb: policy.map(|policy| policy.target_free_gb),
        policy_mode: policy.map(|policy| policy.mode.clone()),
        published_at,
        age_seconds,
        free_slots: payload.map(|p| free_slots(p, target.gpu_type.as_deref())),
        slots_declared: target.slots,
        agent_store_backend: agent_store.map(str::to_string),
        fleet_store_backend: crate::config::wc_storage_backend().to_string(),
        notes,
        local_snapshots,
        waiting_jobs: Vec::new(),
    }
}

/// Queued jobs pinned to this host, oldest first.
///
/// A pinned job names its consumer as `<kind>-<hostname>`, and the hostname
/// is the machine's own word for itself, not its registry name — the same
/// gap [`publication`] closes with [`Registry::lookup_self`], closed the same
/// way here. Jobs pinned by exact registry name are honored too, because the
/// operator-facing `--pinned-host` accepts that spelling.
async fn waiting_jobs(
    registry: &Registry,
    target: &ComputeTarget,
    store: &JobStorage,
    now: DateTime<Utc>,
) -> Result<Vec<WaitingJob>, DeployError> {
    let queued = store
        .list_jobs("queue", 0)
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    let prefix = format!("{}-", target.kind);
    let mut waiting = Vec::new();
    for job in queued {
        if job.pinned_host.is_empty() {
            continue;
        }
        let mine = job.pinned_host == target.name
            || job
                .pinned_host
                .strip_prefix(prefix.as_str())
                .is_some_and(|identity| {
                    registry
                        .lookup_self(identity)
                        .ok()
                        .flatten()
                        .is_some_and(|found| found.name == target.name)
                });
        if !mine {
            continue;
        }
        let age_seconds = DateTime::parse_from_rfc3339(&job.created_at)
            .ok()
            .map(|created| (now - created.with_timezone(&Utc)).num_seconds());
        waiting.push(WaitingJob {
            job_id: job.job_id,
            age_seconds,
        });
    }
    waiting.sort_by_key(|job| std::cmp::Reverse(job.age_seconds));
    Ok(waiting)
}

/// One `diag` boolean the agent published, or `None` when this host published
/// nothing or that tick carried no such key.
fn diag_flag(payload: Option<&Value>, key: &str) -> Option<bool> {
    payload
        .and_then(|payload| payload.get("diag"))
        .and_then(|diag| diag.get(key))
        .and_then(Value::as_bool)
}

/// Free slots of this host's own accelerator shape, not a sum across every
/// sizing tier the broadcast also carries: those entries describe how many
/// jobs of *each* footprint fit, and adding a 2 GB tier to a 48 GB tier to a
/// whole-card count produced "44 free slots of 2 declared" on the RTX PRO
/// 6000. Fall back to the sum only when the broadcast carries no entry named
/// by the host's gpu_type, which is the shape older agents publish.
fn free_slots(payload: &Value, gpu_type: Option<&str>) -> i64 {
    let Some(slots) = payload.get("free_slots").and_then(Value::as_object) else {
        return 0;
    };
    if let Some(gpu_type) = gpu_type {
        if let Some(own) = slots.get(gpu_type).and_then(Value::as_i64) {
            return own;
        }
    }
    slots.values().filter_map(Value::as_i64).sum::<i64>()
}

/// This host's capacity publication, stale ones included.
///
/// [`capacity::read_consumer_capacity`] cannot be used here: it drops
/// everything past the staleness horizon and garbage-collects what is past the
/// GC horizon, which is correct for a scheduler and exactly wrong for the
/// question being asked. A host whose agent went quiet an hour ago is the case
/// this command has to be able to report, not the case it deletes. So the read
/// goes through [`capacity::read_publications`], the one GC-free reader of
/// that prefix, shared with [`super::fleet_claim`] — two readers of
/// `capacity/<consumer>.json` would eventually give two answers to one
/// question, and the operator would believe whichever they ran first.
///
/// The consumer id is `<kind>-<hostname>`
/// ([`crate::providers::local::agent`]), and the hostname a host publishes is
/// its own, which need not be its registry name. So the identity is put back
/// through [`Registry::lookup_self`] — the fleet's one hostname-to-target
/// resolution — and only the row that resolves to THIS target is kept.
async fn publication(
    registry: &Registry,
    target: &ComputeTarget,
    store: &JobStorage,
) -> Result<Option<Publication>, DeployError> {
    let rows = capacity::read_publications(store)
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    for (consumer_id, row) in rows {
        if resolves_to(registry, target, &consumer_id)? {
            return Ok(Some(row));
        }
    }
    Ok(None)
}

/// Whether `consumer_id` — a `<kind>-<hostname>` publication key — names
/// `target`.
///
/// Exported for [`super::fleet_claim`], which asks the same question of every
/// declared host at once and must answer it by exactly this rule.
pub(super) fn resolves_to(
    registry: &Registry,
    target: &ComputeTarget,
    consumer_id: &str,
) -> Result<bool, DeployError> {
    let Some(identity) = consumer_id.strip_prefix(&format!("{}-", target.kind)) else {
        return Ok(false);
    };
    Ok(registry
        .lookup_self(identity)
        .map_err(|exc| DeployError(exc.to_string()))?
        .is_some_and(|found| found.name == target.name))
}

/// The `--json` report, in the exact shape the operator console consumes.
pub fn to_report(gates: &HostGates) -> Map<String, Value> {
    let mut report = Map::new();
    report.insert("host".to_string(), Value::String(gates.host.clone()));
    report.insert("claiming".to_string(), Value::Bool(gates.claiming));
    report.insert(
        "blockers".to_string(),
        Value::Array(
            gates
                .blockers
                .iter()
                .map(|blocker| Value::String(blocker.clone()))
                .collect(),
        ),
    );
    report.insert(
        "notes".to_string(),
        Value::Array(
            gates
                .notes
                .iter()
                .map(|note| Value::String(note.clone()))
                .collect(),
        ),
    );
    report.insert(
        "disk".to_string(),
        json!({
            "free_gb": gates.free_gb,
            "low_watermark_gb": gates.low_watermark_gb,
            "target_free_gb": gates.target_free_gb,
            "policy_mode": gates.policy_mode,
            // Held space no stage of `host reclaim` can return, so an operator
            // reading "free 2 GiB, watermark 55 GiB" is not left to guess.
            // Null on a host that has no such notion at all.
            "local_snapshots": gates.local_snapshots,
            // Whether anything is still trying to keep the two numbers above
            // apart, and how long since it last managed to.
            "cleanup_stalled": gates.disk_cleanup_stalled,
            "cleanup_success_age_seconds": gates.cleanup_success_age_seconds,
            // ...and whether it is not trying because it cannot get the lock,
            // which points at a process and not at this disk.
            "cleanup_lock_held": gates.disk_cleanup_lock_held,
            "cleanup_prevented_age_seconds": gates.cleanup_prevented_age_seconds,
        }),
    );
    report.insert(
        "capacity".to_string(),
        json!({
            "published_at": gates.published_at,
            "age_seconds": gates.age_seconds,
            "free_slots": gates.free_slots,
            "slots_declared": gates.slots_declared,
        }),
    );
    report.insert(
        "store".to_string(),
        json!({
            // Both ends of the sentence, never a boolean verdict: the operator
            // who has to go fix the unit needs the backend name the host
            // resolved, and the one this control plane reads, side by side.
            // `agent_backend` is null on a host that would not answer, which
            // the `agent_store_unreadable` note also says.
            "agent_backend": gates.agent_store_backend,
            "fleet_backend": gates.fleet_store_backend,
        }),
    );
    report.insert(
        "waiting_jobs".to_string(),
        Value::Array(
            gates
                .waiting_jobs
                .iter()
                .map(|job| {
                    json!({
                        "job_id": job.job_id,
                        "age_seconds": job.age_seconds,
                    })
                })
                .collect(),
        ),
    );
    report
}

/// The fields a release verdict embeds when it has to say why a host is not
/// building anything.
///
/// Exported so `stado release doctor` reports the claiming gates from this
/// reader instead of growing a second one: two readers of
/// `capacity/<consumer>.json` would eventually give two answers to one
/// question, and the operator would believe whichever they ran first.
///
/// `disk_cleanup_stalled` rides here beside the pressure and the two numbers
/// because a release verdict is where this fleet actually looks. The host
/// that stopped every release on 2026-09-02 had reported the pressure and the
/// numbers correctly for days; what no verdict anywhere said was that the
/// janitor which was supposed to resolve them had not completed a pass since
/// 2026-08-18.
pub fn gates_section(gates: &HostGates) -> Value {
    json!({
        "disk_pressure_unresolved": gates.disk_pressure_unresolved,
        "disk_cleanup_stalled": gates.disk_cleanup_stalled,
        "disk_cleanup_lock_held": gates.disk_cleanup_lock_held,
        "cleanup_success_age_seconds": gates.cleanup_success_age_seconds,
        "free_gb": gates.free_gb,
        "low_watermark_gb": gates.low_watermark_gb,
    })
}
