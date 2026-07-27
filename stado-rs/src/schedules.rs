//! Recurring (cron) job schedules.
//!
//! Port of `stado/schedules/` (`model.py`, `store.py`, `fire.py`).
//!
//! A Schedule is a recurring job spec — NOT a Job. It is the frozen submit
//! payload (command + sizing/routing kwargs) plus a 5-field cron expression
//! and firing bookkeeping. The coordinator tick evaluates each enabled
//! schedule's `next_due_at` and submits a fresh job when it comes due.
//!
//! Stored at `<bucket>/schedules/<schedule_id>.json`, byte-compatible with
//! Python's `json.dumps(asdict(schedule), indent=2)`.
//!
//! # Cron engine deviation notes (croniter → `cron` crate)
//!
//! Python computes next-fire times with croniter; this port uses the `cron`
//! crate (6/7-field, seconds-first) behind a compat shim that:
//!   * expands a 5-field expression by prepending second `0`,
//!   * translates the day-of-week field from croniter numbering
//!     (0-6 or 0/7 = Sunday) to the crate's 1-7 (Sunday=1),
//!   * maps the croniter-only aliases `@annually`→`@yearly` and
//!     `@midnight`→`@daily` (the crate supports the other five natively),
//!   * replicates the Vixie-cron OR semantics croniter uses when BOTH
//!     day-of-month and day-of-week are restricted, by compiling two
//!     schedules (dom-only and dow-only) and taking the earliest fire —
//!     the crate only knows AND semantics.
//!
//! Corners where the crate CANNOT match croniter (documented, tested):
//!   * spring-forward nonexistent wall times: croniter maps "0 2 * * *" on
//!     the night 02:00 does not exist to 03:00 the same night; the crate
//!     skips to the next day. Fall-back ambiguous times agree (both pick
//!     the first occurrence).
//!   * croniter extensions the crate's parser lacks: `L` (last day of
//!     month) and wrap-around ranges such as `6-1` / `22-2`. These parse
//!     in croniter but fail here, so `cron_is_valid` returns false and the
//!     CLI refuses them at `schedule create` time.

use std::collections::BTreeSet;
use std::str::FromStr;

use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::queue::submit::{submit_job, SubmitOptions};
use crate::queue::{JobStorage, StorageError};

/// Blob prefix holding schedule documents.
pub const PREFIX: &str = "schedules";

/// Cron compilation failure (Python surfaces croniter's `CroniterBadCronError`
/// / `CroniterBadDateError` messages; here the message is ours).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CronError(pub String);

/// `sch-<8 hex>` — namespaced so a schedule id is never confused with a job
/// id in logs (Python `generate_schedule_id`).
pub fn generate_schedule_id() -> String {
    format!("sch-{}", hex::encode(&uuid::Uuid::new_v4().as_bytes()[..4]))
}

// ---------------------------------------------------------------------------
// cron compat shim (croniter 5-field → `cron` crate 6-field)
// ---------------------------------------------------------------------------

/// Raw croniter day-of-week value: 0-6 for names/numbers, 7 for Sunday.
fn raw_dow_value(token: &str) -> Option<u8> {
    match token.to_ascii_lowercase().as_str() {
        "sun" | "sunday" => Some(0),
        "mon" | "monday" => Some(1),
        "tue" | "tues" | "tuesday" => Some(2),
        "wed" | "wednesday" => Some(3),
        "thu" | "thurs" | "thursday" => Some(4),
        "fri" | "friday" => Some(5),
        "sat" | "saturday" => Some(6),
        other => other.parse::<u8>().ok().filter(|n| *n <= 7),
    }
}

/// Expand a croniter day-of-week field to the `cron` crate's 1-7 numbering
/// (Sunday=1) as an explicit comma list. Returns `None` for syntax the shim
/// does not understand (callers then report the expression invalid).
fn expand_dow(field: &str) -> Option<String> {
    let mut days: BTreeSet<u8> = BTreeSet::new();
    for item in field.split(',') {
        let item = item.trim();
        if item.is_empty() {
            return None;
        }
        let (base, step) = match item.split_once('/') {
            Some((base, step)) => (base, step.parse::<u8>().ok().filter(|s| *s > 0)?),
            None => (item, 1),
        };
        if base == "*" || base == "?" {
            for raw in (0u8..7).step_by(step as usize) {
                days.insert(raw);
            }
        } else if let Some((lo, hi)) = base.split_once('-') {
            let lo = raw_dow_value(lo)?;
            let hi = raw_dow_value(hi)?;
            // croniter tolerates wrap-around ranges ("6-1" wraps through
            // Sunday); iterate past 7 and fold with % 7.
            let hi = if hi < lo { hi + 7 } else { hi };
            let mut raw = lo;
            while raw <= hi {
                days.insert(raw % 7);
                raw += step;
            }
        } else {
            let value = raw_dow_value(base)?;
            if step == 1 {
                days.insert(value % 7);
            } else {
                // croniter treats "N/step" as N..6 stepped (e.g. "1/2" =
                // Mon, Wed, Fri).
                let mut raw = value;
                while raw <= 6 {
                    days.insert(raw % 7);
                    raw += step;
                }
            }
        }
    }
    if days.is_empty() {
        return None;
    }
    Some(
        days.iter()
            .map(|day| (day + 1).to_string())
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// True when the field matches every value (croniter treats `?` as `*`).
fn is_all(field: &str) -> bool {
    field == "*" || field == "?"
}

/// Compile a croniter 5-field expression (or @-alias) into one or two
/// `cron` crate schedules. Two schedules appear only for the dom+dow OR
/// case; the effective next-fire is the earliest of the two.
fn compile(cron: &str) -> Result<Vec<cron::Schedule>, CronError> {
    let invalid = || CronError(format!("invalid cron expression: {cron:?}"));
    let expr = cron.trim();
    // Aliases croniter accepts that the crate lacks (the other five —
    // @yearly/@monthly/@weekly/@daily/@hourly — parse natively).
    let expr = match expr {
        "@annually" => "@yearly",
        "@midnight" => "@daily",
        other => other,
    };
    if expr.starts_with('@') {
        return cron::Schedule::from_str(expr)
            .map(|schedule| vec![schedule])
            .map_err(|_| invalid());
    }
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(invalid());
    }
    let (minute, hour, dom, month, dow) = (fields[0], fields[1], fields[2], fields[3], fields[4]);
    let dow_translated = if is_all(dow) {
        "*".to_string()
    } else {
        expand_dow(dow).ok_or_else(invalid)?
    };
    let parse = |expr: String| cron::Schedule::from_str(&expr).map_err(|_| invalid());
    if !is_all(dom) && !is_all(dow) {
        // Vixie-cron OR semantics (croniter): restricted dom ORs with
        // restricted dow. The crate only ANDs, so compile both halves.
        Ok(vec![
            parse(format!("0 {minute} {hour} {dom} {month} *"))?,
            parse(format!("0 {minute} {hour} * {month} {dow_translated}"))?,
        ])
    } else {
        Ok(vec![parse(format!(
            "0 {minute} {hour} {dom} {month} {dow_translated}"
        ))?])
    }
}

/// True iff `cron` parses as a croniter expression (Python `cron_is_valid`).
///
/// See the module docs for the corners where this deliberately diverges
/// from croniter's `is_valid` (`L`, wrap ranges).
pub fn cron_is_valid(cron: &str) -> bool {
    compile(cron).is_ok()
}

/// First cron occurrence strictly after `after_utc`, returned as a UTC
/// datetime (Python `compute_next_due`).
///
/// The cron is interpreted in `tz` (so "0 2 * * *" means 02:00 in that
/// zone, DST included), then converted back to UTC for storage. An
/// unparseable `tz` falls back to UTC, exactly like Python's blanket
/// `except` around `ZoneInfo(tz)`.
pub fn compute_next_due(
    cron: &str,
    after_utc: DateTime<Utc>,
    tz: &str,
) -> Result<DateTime<Utc>, CronError> {
    let zone: Tz = tz.parse().unwrap_or(Tz::UTC);
    let base_local = after_utc.with_timezone(&zone);
    let mut best: Option<DateTime<Tz>> = None;
    for schedule in compile(cron)? {
        if let Some(next) = schedule.after(&base_local).next() {
            best = Some(match best {
                None => next,
                Some(current) => current.min(next),
            });
        }
    }
    best.map(|next| next.with_timezone(&Utc)).ok_or_else(|| {
        CronError(format!(
            "no future occurrence for cron expression: {cron:?}"
        ))
    })
}

/// Python `datetime.fromisoformat` for the shapes our writers produce
/// (offset-aware RFC-3339, or a naive value that gets UTC attached).
fn parse_iso(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
                .map(|naive| naive.and_utc())
                .ok()
        })
}

// ---------------------------------------------------------------------------
// Schedule model (Python schedules/model.py)
// ---------------------------------------------------------------------------

fn default_enabled() -> bool {
    true
}
fn default_tz() -> String {
    "UTC".into()
}
fn default_provider() -> String {
    "gcp".into()
}
fn default_repo_extras() -> String {
    "train".into()
}
fn default_policy() -> String {
    "skip".into()
}

/// A recurring job spec. Field order matches the Python dataclass so the
/// serialized JSON is key-order identical; missing keys fall back to the
/// Python dataclass defaults and unknown keys are ignored (`from_dict`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    #[serde(default)]
    pub schedule_id: String,
    #[serde(default)]
    pub cron: String,
    #[serde(default)]
    pub command: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_tz")]
    pub tz: String,
    // ---- frozen submit kwargs (mirror submit_job's GCS-path params) ----
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub gpu_type: String,
    #[serde(default)]
    pub vram_gb: i64,
    #[serde(default)]
    pub machine_type: String,
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub preemptible: bool,
    #[serde(default)]
    pub max_cost_per_hour_usd: f64,
    #[serde(default)]
    pub pin_to_provider: bool,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub repo_workdir: String,
    #[serde(default = "default_repo_extras")]
    pub repo_extras: String,
    #[serde(default)]
    pub pre_command: String,
    #[serde(default)]
    pub apt_packages: Vec<String>,
    #[serde(default)]
    pub output_uri: String,
    #[serde(default)]
    pub verify_command: String,
    #[serde(default)]
    pub exclusive: bool,
    // ---- firing bookkeeping ----
    /// Filled by [`Schedule::finalize_new`] when empty (Python
    /// `__post_init__`).
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub created_by: String,
    /// ISO-8601 UTC; "" disables firing.
    #[serde(default)]
    pub next_due_at: String,
    #[serde(default)]
    pub last_fired_at: Option<String>,
    #[serde(default)]
    pub last_run_id: String,
    /// Most recent fire's job_id (overlap-skip check).
    #[serde(default)]
    pub last_job_id: String,
    #[serde(default)]
    pub fire_count: i64,
    /// skip: do not fire while last_job_id is still in queue/ or running/.
    /// allow: fire regardless of prior instance.
    #[serde(default = "default_policy")]
    pub overlap_policy: String,
    /// skip: a coordinator-downtime gap collapses to a single fire and
    ///   next_due_at jumps to the next future occurrence.
    /// each: not yet honored beyond skip — reserved (see fire docs).
    #[serde(default = "default_policy")]
    pub catchup_policy: String,
}

impl Schedule {
    /// Python `__post_init__`: stamp `created_at` when empty.
    pub fn finalize_new(&mut self) {
        if self.created_at.is_empty() {
            self.created_at = crate::models::isoformat_utc(Utc::now());
        }
    }

    /// `Schedule(...)` with Python dataclass defaults.
    pub fn new(
        schedule_id: impl Into<String>,
        cron: impl Into<String>,
        command: impl Into<String>,
    ) -> Self {
        let mut sched: Schedule =
            serde_json::from_value(serde_json::Value::Object(serde_json::Map::new()))
                .expect("all fields have serde defaults");
        sched.schedule_id = schedule_id.into();
        sched.cron = cron.into();
        sched.command = command.into();
        sched.finalize_new();
        sched
    }

    /// The kwargs to hand `submit_job` for one fire of this schedule
    /// (Python `submit_kwargs`).
    pub fn submit_options(&self) -> SubmitOptions {
        SubmitOptions {
            provider: self.provider.clone(),
            gpu_type: self.gpu_type.clone(),
            vram_gb: self.vram_gb,
            machine_type: self.machine_type.clone(),
            priority: self.priority,
            preemptible: self.preemptible,
            max_cost_per_hour_usd: self.max_cost_per_hour_usd,
            pin_to_provider: self.pin_to_provider,
            repo: self.repo.clone(),
            repo_workdir: self.repo_workdir.clone(),
            repo_extras: self.repo_extras.clone(),
            pre_command: self.pre_command.clone(),
            apt_packages: self.apt_packages.clone(),
            output_uri: self.output_uri.clone(),
            verify_command: self.verify_command.clone(),
            exclusive: self.exclusive,
            ..Default::default()
        }
    }

    /// Byte-compatible with Python `json.dumps(asdict(schedule), indent=2)`
    /// (ensure_ascii=True).
    pub fn to_json(&self) -> String {
        let pretty =
            serde_json::to_string_pretty(self).expect("Schedule serialization is infallible");
        crate::models::ensure_ascii(&pretty)
    }

    /// Python `Schedule.from_json` / `from_dict`: unknown keys ignored,
    /// missing keys defaulted, `created_at` post-init applied.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        let mut sched: Self = serde_json::from_str(s)?;
        sched.finalize_new();
        Ok(sched)
    }
}

// ---------------------------------------------------------------------------
// store (Python schedules/store.py)
//
// Reuses JobStorage's prefix-agnostic blob helpers for the common paths.
// The one special case is claim_due(): an atomic compare-and-set on
// next_due_at, so two overlapping coordinator invocations can never
// double-fire the same occurrence.
// ---------------------------------------------------------------------------

fn path(schedule_id: &str) -> String {
    format!("{PREFIX}/{schedule_id}.json")
}

/// Fresh, generation-pinned read of `path` (Python `_read_fresh_text`).
///
/// A plain no-generation download on the wisent-compute bucket can return
/// a stale (edge-cached) copy of an object that was just overwritten in
/// place — confirmed live 2026-06-01: a schedule's next_due_at update read
/// back as the OLD value via `store._download_text` even though the new
/// generation was already the latest. The existing queue never hit this
/// because it is write-once-then-delete; schedules overwrite the same blob
/// every tick (read-modify-write of next_due_at), which is exactly the
/// pattern the cache breaks. `read_text_versioned` fetches the current
/// generation first and pins the download to it, so the bytes are
/// guaranteed to be the latest. (Python falls back to a plain read on the
/// gsutil/Azure paths; our local backend's versioned read is a locked
/// content read, which is already fresh.)
async fn read_fresh_text(store: &JobStorage, path: &str) -> Result<Option<String>, StorageError> {
    Ok(store
        .read_text_versioned(path)
        .await?
        .map(|versioned| versioned.content))
}

/// Schedule ids of every `schedules/<id>.json` blob.
pub async fn list_schedule_ids(store: &JobStorage) -> Result<Vec<String>, StorageError> {
    let paths = store.list_paths(&format!("{PREFIX}/"), 0).await?;
    Ok(paths
        .iter()
        .filter_map(|name| name.rsplit('/').next())
        .filter(|base| base.ends_with(".json"))
        .map(|base| base[..base.len() - ".json".len()].to_string())
        .collect())
}

/// Read one schedule; `None` when it does not exist.
pub async fn read_schedule(
    store: &JobStorage,
    schedule_id: &str,
) -> Result<Option<Schedule>, StorageError> {
    let Some(data) = read_fresh_text(store, &path(schedule_id)).await? else {
        return Ok(None);
    };
    Ok(Some(Schedule::from_json(&data)?))
}

/// Every schedule, in listing order.
pub async fn list_schedules(store: &JobStorage) -> Result<Vec<Schedule>, StorageError> {
    let mut out = Vec::new();
    for schedule_id in list_schedule_ids(store).await? {
        if let Some(sched) = read_schedule(store, &schedule_id).await? {
            out.push(sched);
        }
    }
    Ok(out)
}

/// Unconditional overwrite of the schedule blob.
pub async fn write_schedule(store: &JobStorage, sched: &Schedule) -> Result<(), StorageError> {
    store
        .upload_text(&path(&sched.schedule_id), &sched.to_json())
        .await
}

/// Delete the schedule blob; `false` when it did not exist.
pub async fn delete_schedule(store: &JobStorage, schedule_id: &str) -> Result<bool, StorageError> {
    if read_schedule(store, schedule_id).await?.is_none() {
        return Ok(false);
    }
    store.delete_blob(&path(schedule_id)).await?;
    Ok(true)
}

/// Advance `sched.next_due_at` to `new_next_due_at` and persist, but only
/// if no other writer has touched the blob since it was read (Python
/// `claim_due`'s GCS `if_generation_match` precondition).
///
/// Returns `true` if THIS caller won the claim (and should now submit the
/// job), `false` if a concurrent coordinator already advanced it. A blob
/// that vanished since the read is recreated create-only (`if_generation_
/// match=0` in Python), which also loses the race to a concurrent creator.
pub async fn claim_due(
    store: &JobStorage,
    sched: &mut Schedule,
    new_next_due_at: &str,
) -> Result<bool, StorageError> {
    sched.next_due_at = new_next_due_at.to_string();
    let body = sched.to_json();
    let path = path(&sched.schedule_id);
    match store.read_text_versioned(&path).await? {
        None => store.create_text_if_absent(&path, &body).await,
        Some(versioned) => match store
            .compare_and_swap_text(&path, &versioned.version, &body)
            .await
        {
            Ok(_) => Ok(true),
            Err(StorageError::StorageConflict(_)) => Ok(false),
            // Deleted between the versioned read and the CAS: Python's
            // generation-0 upload creates it only when it still does not
            // exist.
            Err(StorageError::NotFound(_)) => store.create_text_if_absent(&path, &body).await,
            Err(exc) => Err(exc),
        },
    }
}

// ---------------------------------------------------------------------------
// fire (Python schedules/fire.py)
//
// A schedule is "due" when now >= next_due_at. Firing is:
//   1. compute the next future occurrence,
//   2. atomically claim it (claim_due — advances next_due_at FIRST, under
//      a generation match, so an overlapping invocation can't double-fire),
//   3. submit the job tagged with schedule_id + a fresh run_id,
//   4. record last_fired_at / last_run_id / last_job_id / fire_count.
//
// catchup_policy is "skip" only for now: if the coordinator was down across
// several occurrences, step 1 jumps straight to the next future slot, so
// the backlog collapses to a single fire rather than a burst.
// ---------------------------------------------------------------------------

/// True iff this schedule's most recent fire is still queued/running.
/// Two direct reads by id — cheap, unlike scanning queue/ (14k+ blobs).
async fn prev_instance_live(store: &JobStorage, sched: &Schedule) -> bool {
    if sched.last_job_id.is_empty() {
        return false;
    }
    let live = async {
        Ok::<bool, StorageError>(
            store.read_job("queue", &sched.last_job_id).await?.is_some()
                || store
                    .read_job("running", &sched.last_job_id)
                    .await?
                    .is_some(),
        )
    };
    // A read error is not proof the prior instance is gone; be
    // conservative and treat it as live so overlap_policy=skip holds.
    live.await.unwrap_or(true)
}

/// Fire every due+enabled schedule once. Returns the number fired.
pub async fn fire_due_schedules(
    store: &JobStorage,
    mut log: impl FnMut(&str),
    now: DateTime<Utc>,
) -> Result<i64, StorageError> {
    let mut fired = 0;
    for schedule_id in list_schedule_ids(store).await? {
        let Some(mut sched) = read_schedule(store, &schedule_id).await? else {
            continue;
        };
        if !sched.enabled || sched.next_due_at.is_empty() {
            continue;
        }
        let Some(due) = parse_iso(&sched.next_due_at) else {
            log(&format!(
                "schedule {schedule_id}: unparseable next_due_at={:?}; skipping",
                sched.next_due_at
            ));
            continue;
        };
        if due > now {
            continue;
        }

        // Python lets a bad cron here crash the tick (the expression was
        // validated at create); propagate for the same fail-loud behavior.
        let next_due = crate::models::isoformat_utc(
            compute_next_due(&sched.cron, now, &sched.tz)
                .map_err(|exc| StorageError::Other(exc.to_string()))?,
        );

        if sched.overlap_policy == "skip" && prev_instance_live(store, &sched).await {
            // Don't fire on top of a still-running prior instance — but DO
            // advance next_due_at so we re-evaluate cleanly next tick
            // instead of re-firing the same overdue slot every tick.
            sched.next_due_at = next_due;
            write_schedule(store, &sched).await?;
            log(&format!(
                "schedule {schedule_id}: skip fire (prior job {} still live)",
                sched.last_job_id
            ));
            continue;
        }

        // Claim the occurrence before submitting (CF double-fire guard).
        if !claim_due(store, &mut sched, &next_due).await? {
            log(&format!(
                "schedule {schedule_id}: lost claim race; another coordinator fired it"
            ));
            continue;
        }

        let run_id = crate::queue::runs::generate_run_id();
        let options = SubmitOptions {
            bucket: store.bucket_name().to_string(),
            run_id: run_id.clone(),
            schedule_id: schedule_id.clone(),
            ..sched.submit_options()
        };
        let job = match submit_job(&sched.command, &options).await {
            Ok(job) => job,
            Err(exc) => {
                // next_due_at is already advanced (occurrence consumed).
                // Record the miss rather than retry-storming; the next
                // occurrence fires normally.
                log(&format!(
                    "schedule {schedule_id}: submit FAILED (SubmitError: {exc}); occurrence skipped"
                ));
                continue;
            }
        };

        sched.last_fired_at = Some(crate::models::isoformat_utc(now));
        sched.last_run_id = run_id.clone();
        sched.last_job_id = job.job_id.clone();
        sched.fire_count += 1;
        write_schedule(store, &sched).await?;
        fired += 1;
        log(&format!(
            "schedule {schedule_id}: fired job {} (run {run_id}); next_due={next_due}",
            job.job_id
        ));
    }
    Ok(fired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn schedule_id_format() {
        let id = generate_schedule_id();
        assert!(id.starts_with("sch-"));
        assert_eq!(id.len(), 12);
        assert!(id[4..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn model_defaults_and_round_trip() {
        let sched = Schedule::new("sch-deadbeef", "0 2 * * *", "echo hi");
        assert!(sched.enabled);
        assert_eq!(sched.tz, "UTC");
        assert_eq!(sched.provider, "gcp");
        assert_eq!(sched.repo_extras, "train");
        assert_eq!(sched.overlap_policy, "skip");
        assert_eq!(sched.catchup_policy, "skip");
        assert_eq!(sched.last_fired_at, None);
        assert!(!sched.created_at.is_empty());
        let back = Schedule::from_json(&sched.to_json()).unwrap();
        assert_eq!(back.to_json(), sched.to_json());
        // from_dict tolerance: unknown keys ignored, missing defaulted.
        let tolerant = Schedule::from_json(
            r#"{"schedule_id": "sch-x", "cron": "* * * * *", "command": "c", "bogus": 1}"#,
        )
        .unwrap();
        assert_eq!(tolerant.tz, "UTC");
        assert!(tolerant.enabled);
    }

    #[test]
    fn cron_validity() {
        for valid in [
            "* * * * *",
            "*/5 * * * *",
            "0 2 * * *",
            "30 14 * * 1-5",
            "0 22 * * 7",
            "0 6 * * mon,wed,fri",
            "0 12 29 2 *",
            "0 0 13 * 5",
            "0 3 15 jan,jun *",
            "0 0 ? * 1",
            "0 0 * * */2",
            "0 0 * * 1/2",
            "0 0 * * 6-1",
            "@daily",
            "@hourly",
            "@weekly",
            "@monthly",
            "@yearly",
            "@annually",
            "@midnight",
        ] {
            assert!(cron_is_valid(valid), "{valid} should be valid");
        }
        for invalid in [
            "",
            "0 0 0 * *",
            "0 25 * * *",
            "61 * * * *",
            "* * *",
            "* * * * * *",
            "0 0 * * 8",
            "0 0 * * someday",
            "@reboot",
            "@sometimes",
            // croniter parses these; the `cron` crate cannot (documented
            // deviation — refused at create time).
            "0 0 L * *",
            "0 22-2 * * *",
        ] {
            assert!(!cron_is_valid(invalid), "{invalid} should be invalid");
        }
    }

    /// croniter parity battery: expected next-fire UTC times generated with
    /// `/opt/homebrew/Caskroom/miniforge/base/bin/python3` + croniter (the
    /// same call compute_next_due makes in Python). (cron, tz, base_utc,
    /// expected_utc).
    const CRONITER_BATTERY: [(&str, &str, &str, &str); 21] = [
        (
            "*/5 * * * *",
            "UTC",
            "2026-01-15T10:03:27+00:00",
            "2026-01-15T10:05:00+00:00",
        ),
        (
            "0 2 * * *",
            "UTC",
            "2026-01-15T10:03:27+00:00",
            "2026-01-16T02:00:00+00:00",
        ),
        (
            "30 14 * * 1-5",
            "UTC",
            "2026-07-26T00:00:00+00:00",
            "2026-07-27T14:30:00+00:00",
        ),
        (
            "0 0 1 * *",
            "UTC",
            "2026-07-25T03:42:00+00:00",
            "2026-08-01T00:00:00+00:00",
        ),
        (
            "0 9 1,15 * *",
            "UTC",
            "2026-07-15T09:00:00+00:00",
            "2026-08-01T09:00:00+00:00",
        ),
        (
            "0 22 * * 0",
            "UTC",
            "2026-07-25T00:00:00+00:00",
            "2026-07-26T22:00:00+00:00",
        ),
        (
            "0 22 * * 7",
            "UTC",
            "2026-07-25T00:00:00+00:00",
            "2026-07-26T22:00:00+00:00",
        ),
        (
            "0 6 * * mon,wed,fri",
            "UTC",
            "2026-07-26T00:00:00+00:00",
            "2026-07-27T06:00:00+00:00",
        ),
        (
            "0 12 29 2 *",
            "UTC",
            "2026-07-25T00:00:00+00:00",
            "2028-02-29T12:00:00+00:00",
        ),
        (
            "0 0 13 * 5",
            "UTC",
            "2026-07-25T00:00:00+00:00",
            "2026-07-31T00:00:00+00:00",
        ),
        (
            "*/20 1-4 * * *",
            "UTC",
            "2026-07-25T02:41:11+00:00",
            "2026-07-25T03:00:00+00:00",
        ),
        (
            "0 3 15 jan,jun *",
            "UTC",
            "2026-07-25T00:00:00+00:00",
            "2027-01-15T03:00:00+00:00",
        ),
        (
            "15,45 8-18/3 * * *",
            "UTC",
            "2026-07-25T11:46:00+00:00",
            "2026-07-25T14:15:00+00:00",
        ),
        // DST fall-back (ambiguous local time): both pick the FIRST occurrence.
        (
            "0 2 * * *",
            "Europe/Warsaw",
            "2026-10-24T12:00:00+00:00",
            "2026-10-25T00:00:00+00:00",
        ),
        (
            "30 1 * * *",
            "America/New_York",
            "2026-11-01T04:30:00+00:00",
            "2026-11-01T05:30:00+00:00",
        ),
        (
            "0 9 * * 1",
            "Asia/Tokyo",
            "2026-07-25T00:00:00+00:00",
            "2026-07-27T00:00:00+00:00",
        ),
        (
            "30 23 * * sat",
            "Europe/Warsaw",
            "2026-07-25T00:00:00+00:00",
            "2026-07-25T21:30:00+00:00",
        ),
        (
            "0 0 29 2 *",
            "UTC",
            "2027-03-01T00:00:00+00:00",
            "2028-02-29T00:00:00+00:00",
        ),
        // croniter DOW step forms normalized through the shim.
        (
            "0 22 * * */2",
            "UTC",
            "2026-07-25T00:00:00+00:00",
            "2026-07-25T22:00:00+00:00",
        ),
        (
            "0 22 * * 1/2",
            "UTC",
            "2026-07-25T00:00:00+00:00",
            "2026-07-27T22:00:00+00:00",
        ),
        // croniter DOW wrap-around range (Sat,Sun,Mon).
        (
            "0 22 * * 6-1",
            "UTC",
            "2026-07-25T00:00:00+00:00",
            "2026-07-25T22:00:00+00:00",
        ),
    ];

    #[test]
    fn next_due_matches_croniter() {
        for (cron, tz, base, expected) in CRONITER_BATTERY {
            let actual = compute_next_due(cron, utc(base), tz)
                .unwrap_or_else(|exc| panic!("{cron} in {tz} failed: {exc}"));
            assert_eq!(
                crate::models::isoformat_utc(actual),
                expected,
                "cron {cron:?} in {tz} after {base}"
            );
        }
    }

    /// DOCUMENTED DEVIATION: spring-forward nonexistent wall times.
    /// croniter maps "0 2 * * *" to 03:00 on the night 02:00 does not
    /// exist; the `cron` crate skips the whole day and fires the next
    /// night. Pinned here so the divergence is explicit, not accidental.
    #[test]
    fn spring_forward_deviation_is_pinned() {
        // croniter: 2026-03-29T01:00:00+00:00 (03:00 CEST on 03-29).
        let actual = compute_next_due(
            "0 2 * * *",
            utc("2026-03-28T12:00:00+00:00"),
            "Europe/Warsaw",
        )
        .unwrap();
        assert_eq!(
            crate::models::isoformat_utc(actual),
            "2026-03-30T00:00:00+00:00"
        );
        // croniter: 2026-03-08T07:00:00+00:00 (03:00 EDT on 03-08).
        let actual = compute_next_due(
            "0 2 * * *",
            utc("2026-03-07T12:00:00+00:00"),
            "America/New_York",
        )
        .unwrap();
        assert_eq!(
            crate::models::isoformat_utc(actual),
            "2026-03-09T06:00:00+00:00"
        );
    }

    #[test]
    fn unknown_tz_falls_back_to_utc() {
        let actual =
            compute_next_due("0 2 * * *", utc("2026-01-15T10:03:27+00:00"), "Not/AZone").unwrap();
        assert_eq!(
            crate::models::isoformat_utc(actual),
            "2026-01-16T02:00:00+00:00"
        );
    }

    #[tokio::test]
    async fn store_round_trip_and_ids() {
        let (_dir, store) = store();
        let sched = Schedule::new("sch-aa11bb22", "0 2 * * *", "echo hi");
        write_schedule(&store, &sched).await.unwrap();
        let back = read_schedule(&store, "sch-aa11bb22")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(back.to_json(), sched.to_json());
        assert_eq!(
            list_schedule_ids(&store).await.unwrap(),
            vec!["sch-aa11bb22"]
        );
        assert!(read_schedule(&store, "sch-nope").await.unwrap().is_none());
        assert!(delete_schedule(&store, "sch-aa11bb22").await.unwrap());
        assert!(!delete_schedule(&store, "sch-aa11bb22").await.unwrap());
    }

    #[tokio::test]
    async fn claim_due_wins_uncontested_and_loses_stale_version() {
        let (_dir, store) = store();
        let mut sched = Schedule::new("sch-claim01", "0 2 * * *", "echo hi");
        write_schedule(&store, &sched).await.unwrap();

        // A concurrent writer advances the blob after our read.
        let mut raced = read_schedule(&store, "sch-claim01").await.unwrap().unwrap();
        raced.fire_count = 99;
        write_schedule(&store, &raced).await.unwrap();

        // Our claim is based on a versioned read taken NOW, so it wins —
        // matching Python, where claim_due re-reads the generation at
        // claim time.
        assert!(claim_due(&store, &mut sched, "2026-01-16T02:00:00+00:00")
            .await
            .unwrap());
        let back = read_schedule(&store, "sch-claim01").await.unwrap().unwrap();
        assert_eq!(back.next_due_at, "2026-01-16T02:00:00+00:00");

        // Simulate a lost race by CAS-ing with a stale version directly.
        let versioned = store
            .read_text_versioned(&path("sch-claim01"))
            .await
            .unwrap()
            .unwrap();
        store
            .compare_and_swap_text(&path("sch-claim01"), &versioned.version, &raced.to_json())
            .await
            .unwrap();
        let err = store
            .compare_and_swap_text(&path("sch-claim01"), &versioned.version, &sched.to_json())
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::StorageConflict(_)), "{err:?}");
    }

    #[test]
    fn parse_iso_accepts_aware_and_naive() {
        assert_eq!(
            parse_iso("2026-01-16T02:00:00+00:00").unwrap(),
            utc("2026-01-16T02:00:00+00:00")
        );
        assert_eq!(
            parse_iso("2026-01-16T02:00:00").unwrap(),
            utc("2026-01-16T02:00:00+00:00")
        );
        assert_eq!(
            parse_iso("2026-01-16T02:00:00Z").unwrap(),
            utc("2026-01-16T02:00:00+00:00")
        );
        assert!(parse_iso("garbage").is_none());
    }
}
