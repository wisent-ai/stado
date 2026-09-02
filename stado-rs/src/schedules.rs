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

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::models::JobSecretRef;
use crate::queue::submit::{stable_run_id, submit_batch, SubmitOptions};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleOccurrenceReservation {
    pub occurrence_key: String,
    pub occurrence_at: String,
    pub run_id: String,
    pub state: String,
    pub owner: String,
    pub lease_expires_at: String,
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
    /// Durable deletion tombstone; retained so deletion cannot erase a claimed
    /// occurrence between reservation and enqueue.
    #[serde(default)]
    pub deleted: bool,
    #[serde(default = "default_tz")]
    pub tz: String,
    // ---- frozen durable submission options ----
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
    pub pinned_host: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub repo_ref: String,
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
    #[serde(default)]
    pub secret_env: BTreeMap<String, JobSecretRef>,
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
    /// Durable occurrence reservation. It is written in the same CAS that
    /// advances next_due_at, then cleared only after durable run acceptance.
    #[serde(default)]
    pub pending_occurrence: Option<ScheduleOccurrenceReservation>,
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

    /// The frozen options bound into one occurrence's durable run manifest.
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
            pinned_host: self.pinned_host.clone(),
            repo: self.repo.clone(),
            repo_ref: self.repo_ref.clone(),
            repo_workdir: self.repo_workdir.clone(),
            repo_extras: self.repo_extras.clone(),
            pre_command: self.pre_command.clone(),
            apt_packages: self.apt_packages.clone(),
            output_uri: self.output_uri.clone(),
            verify_command: self.verify_command.clone(),
            exclusive: self.exclusive,
            secret_env: self.secret_env.clone(),
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
            if !sched.deleted {
                out.push(sched);
            }
        }
    }
    Ok(out)
}

/// Create a schedule exactly once. Mutable bookkeeping/configuration uses
/// dedicated CAS updates so it cannot erase a pending occurrence reservation.
pub async fn write_schedule(store: &JobStorage, sched: &Schedule) -> Result<(), StorageError> {
    if store
        .create_text_if_absent(&path(&sched.schedule_id), &sched.to_json())
        .await?
    {
        Ok(())
    } else {
        Err(StorageError::StorageConflict(format!(
            "schedule {} already exists",
            sched.schedule_id
        )))
    }
}

/// CAS-update enablement while preserving any pending occurrence lease.
pub async fn set_schedule_enabled(
    store: &JobStorage,
    schedule_id: &str,
    enabled: bool,
    next_due_at: Option<&str>,
) -> Result<Option<Schedule>, StorageError> {
    let path = path(schedule_id);
    for _ in 0..16 {
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            return Ok(None);
        };
        let mut sched = Schedule::from_json(&versioned.content)?;
        if sched.deleted {
            return Ok(None);
        }
        sched.enabled = enabled;
        if sched.pending_occurrence.is_none() {
            if let Some(next_due_at) = next_due_at {
                sched.next_due_at = next_due_at.to_string();
            }
        }
        match store
            .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
            .await
        {
            Ok(_) => return Ok(Some(sched)),
            Err(StorageError::StorageConflict(_)) => continue,
            Err(StorageError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    Err(StorageError::StorageConflict(format!(
        "schedule {schedule_id} remained contended during enablement update"
    )))
}

/// CAS-write a durable deletion tombstone; `false` when absent/already deleted.
pub async fn delete_schedule(store: &JobStorage, schedule_id: &str) -> Result<bool, StorageError> {
    let path = path(schedule_id);
    for _ in 0..16 {
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            return Ok(false);
        };
        let mut sched = Schedule::from_json(&versioned.content)?;
        if sched.deleted {
            return Ok(false);
        }
        sched.deleted = true;
        sched.enabled = false;
        match store
            .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
            .await
        {
            Ok(_) => return Ok(true),
            Err(StorageError::StorageConflict(_)) => continue,
            Err(StorageError::NotFound(_)) => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    Err(StorageError::StorageConflict(format!(
        "schedule {schedule_id} remained contended during deletion"
    )))
}

fn occurrence_token(schedule_id: &str, occurrence_at: &str) -> String {
    format!("{schedule_id}\0{occurrence_at}")
}

fn occurrence_lease_live(reservation: &ScheduleOccurrenceReservation) -> bool {
    DateTime::parse_from_rfc3339(&reservation.lease_expires_at)
        .ok()
        .is_some_and(|expires| expires > Utc::now())
}

async fn reserve_due_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    occurrence_at: &str,
    new_next_due_at: &str,
    owner: &str,
) -> Result<Option<Schedule>, StorageError> {
    let path = path(schedule_id);
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let mut sched = Schedule::from_json(&versioned.content)?;
    if sched.deleted {
        return Ok(None);
    }
    if sched.pending_occurrence.is_some() || sched.next_due_at != occurrence_at {
        return Ok(None);
    }
    let token = occurrence_token(schedule_id, occurrence_at);
    sched.next_due_at = new_next_due_at.to_string();
    sched.pending_occurrence = Some(ScheduleOccurrenceReservation {
        occurrence_key: stable_run_id("schedule-occurrence", &token),
        occurrence_at: occurrence_at.to_string(),
        run_id: stable_run_id("schedule", &token),
        state: "claimed".into(),
        owner: owner.to_string(),
        lease_expires_at: (Utc::now() + chrono::Duration::minutes(15)).to_rfc3339(),
    });
    match store
        .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
        .await
    {
        Ok(_) => Ok(Some(sched)),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn reserve_manual_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    owner: &str,
    retry_token: &str,
) -> Result<Option<Schedule>, StorageError> {
    let path = path(schedule_id);
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let mut sched = Schedule::from_json(&versioned.content)?;
    if sched.deleted {
        return Ok(None);
    }
    if sched.pending_occurrence.is_some() {
        return Ok(None);
    }
    let token = format!("{schedule_id}\0{retry_token}");
    let occurrence_at = format!("manual:{}", stable_run_id("schedule-manual", &token));
    sched.pending_occurrence = Some(ScheduleOccurrenceReservation {
        occurrence_key: stable_run_id("schedule-occurrence", &token),
        occurrence_at,
        run_id: stable_run_id("schedule", &token),
        state: "claimed".into(),
        owner: owner.to_string(),
        lease_expires_at: (Utc::now() + chrono::Duration::minutes(15)).to_rfc3339(),
    });
    match store
        .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
        .await
    {
        Ok(_) => Ok(Some(sched)),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn takeover_pending_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    owner: &str,
) -> Result<Option<Schedule>, StorageError> {
    let path = path(schedule_id);
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let mut sched = Schedule::from_json(&versioned.content)?;
    let Some(pending) = sched.pending_occurrence.as_mut() else {
        return Ok(None);
    };
    if occurrence_lease_live(pending) && pending.owner != owner {
        return Ok(None);
    }
    pending.state = "claimed".into();
    pending.owner = owner.to_string();
    pending.lease_expires_at =
        (Utc::now() + chrono::Duration::minutes(15)).to_rfc3339();
    match store
        .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
        .await
    {
        Ok(_) => Ok(Some(sched)),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn begin_pending_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    occurrence_key: &str,
    owner: &str,
) -> Result<Option<Schedule>, StorageError> {
    let path = path(schedule_id);
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let mut sched = Schedule::from_json(&versioned.content)?;
    let Some(pending) = sched.pending_occurrence.as_mut() else {
        return Ok(None);
    };
    if pending.occurrence_key != occurrence_key
        || pending.owner != owner
        || pending.state != "claimed"
    {
        return Ok(None);
    }
    pending.state = "enqueuing".into();
    match store
        .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
        .await
    {
        Ok(_) => Ok(Some(sched)),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn release_pending_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    occurrence_key: &str,
    owner: &str,
) {
    let path = path(schedule_id);
    let Ok(Some(versioned)) = store.read_text_versioned(&path).await else {
        return;
    };
    let Ok(mut sched) = Schedule::from_json(&versioned.content) else {
        return;
    };
    let Some(pending) = sched.pending_occurrence.as_mut() else {
        return;
    };
    if pending.occurrence_key != occurrence_key || pending.owner != owner {
        return;
    }
    pending.state = "claimed".into();
    pending.owner.clear();
    pending.lease_expires_at = Utc::now().to_rfc3339();
    let _ = store
        .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
        .await;
}

async fn accept_pending_occurrence(
    store: &JobStorage,
    schedule_id: &str,
    occurrence_key: &str,
    owner: &str,
    job: &crate::models::Job,
    fired_at: DateTime<Utc>,
) -> Result<bool, StorageError> {
    let path = path(schedule_id);
    for _ in 0..16 {
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            return Ok(false);
        };
        let mut sched = Schedule::from_json(&versioned.content)?;
        let Some(pending) = sched.pending_occurrence.as_ref() else {
            return Ok(sched.last_run_id == job.run_id && sched.last_job_id == job.job_id);
        };
        if pending.occurrence_key != occurrence_key
            || pending.owner != owner
            || pending.state != "enqueuing"
            || pending.run_id != job.run_id
        {
            return Ok(false);
        }
        sched.last_fired_at = Some(crate::models::isoformat_utc(fired_at));
        sched.last_run_id = job.run_id.clone();
        sched.last_job_id = job.job_id.clone();
        sched.fire_count += 1;
        sched.pending_occurrence = None;
        match store
            .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
            .await
        {
            Ok(_) => return Ok(true),
            Err(StorageError::StorageConflict(_)) => continue,
            Err(StorageError::NotFound(_)) => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    Err(StorageError::StorageConflict(format!(
        "schedule {schedule_id} remained contended while accepting occurrence"
    )))
}

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

async fn enqueue_pending_occurrence(
    store: &JobStorage,
    sched: Schedule,
    owner: &str,
    fired_at: DateTime<Utc>,
) -> Result<Option<crate::models::Job>, StorageError> {
    let pending = sched.pending_occurrence.clone().ok_or_else(|| {
        StorageError::Other(format!(
            "schedule {} has no pending occurrence",
            sched.schedule_id
        ))
    })?;
    let Some(sched) = begin_pending_occurrence(
        store,
        &sched.schedule_id,
        &pending.occurrence_key,
        owner,
    )
    .await?
    else {
        return Ok(None);
    };
    let pending = sched
        .pending_occurrence
        .clone()
        .expect("begin preserves pending occurrence");
    let options = SubmitOptions {
        bucket: store.bucket_name().to_string(),
        run_id: pending.run_id.clone(),
        schedule_id: sched.schedule_id.clone(),
        ..sched.submit_options()
    };
    let commands = [sched.command.clone()];
    let mut jobs = match submit_batch(&commands, &options).await {
        Ok(jobs) => jobs,
        Err(error) => {
            release_pending_occurrence(
                store,
                &sched.schedule_id,
                &pending.occurrence_key,
                owner,
            )
            .await;
            return Err(StorageError::Other(format!(
                "durable schedule enqueue failed: {error}"
            )));
        }
    };
    let job = jobs.pop().ok_or_else(|| {
        StorageError::Other("durable schedule enqueue returned no job".into())
    })?;
    if !accept_pending_occurrence(
        store,
        &sched.schedule_id,
        &pending.occurrence_key,
        owner,
        &job,
        fired_at,
    )
    .await?
    {
        return Err(StorageError::StorageConflict(format!(
            "schedule {} ownership changed after durable enqueue",
            sched.schedule_id
        )));
    }
    Ok(Some(job))
}

async fn advance_due_without_work(
    store: &JobStorage,
    schedule_id: &str,
    expected_due_at: &str,
    new_next_due_at: &str,
) -> Result<bool, StorageError> {
    let path = path(schedule_id);
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(false);
    };
    let mut sched = Schedule::from_json(&versioned.content)?;
    if sched.pending_occurrence.is_some() || sched.next_due_at != expected_due_at {
        return Ok(false);
    }
    sched.next_due_at = new_next_due_at.to_string();
    match store
        .compare_and_swap_text(&path, &versioned.version, &sched.to_json())
        .await
    {
        Ok(_) => Ok(true),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Ok(false),
        Err(error) => Err(error),
    }
}


/// Fire every due+enabled schedule once. Returns the number fired.
pub async fn fire_due_schedules(
    store: &JobStorage,
    mut log: impl FnMut(&str),
    now: DateTime<Utc>,
) -> Result<i64, StorageError> {
    let mut fired = 0;
    for schedule_id in list_schedule_ids(store).await? {
        let Some(sched) = read_schedule(store, &schedule_id).await? else {
            continue;
        };
        let owner = uuid::Uuid::new_v4().simple().to_string();
        if sched.pending_occurrence.is_some() {
            let Some(claimed) =
                takeover_pending_occurrence(store, &schedule_id, &owner).await?
            else {
                log(&format!(
                    "schedule {schedule_id}: pending occurrence is leased by another coordinator"
                ));
                continue;
            };
            match enqueue_pending_occurrence(store, claimed, &owner, now).await {
                Ok(Some(job)) => {
                    fired += 1;
                    log(&format!(
                        "schedule {schedule_id}: recovered durable occurrence as job {} (run {})",
                        job.job_id, job.run_id
                    ));
                }
                Ok(None) => log(&format!(
                    "schedule {schedule_id}: lost pending occurrence ownership"
                )),
                Err(error) => log(&format!(
                    "schedule {schedule_id}: pending occurrence enqueue failed: {error}"
                )),
            }
            continue;
        }
        if !sched.enabled || sched.next_due_at.is_empty() {
            continue;
        }
        let occurrence_at = sched.next_due_at.clone();
        let Some(due) = parse_iso(&occurrence_at) else {
            log(&format!(
                "schedule {schedule_id}: unparseable next_due_at={occurrence_at:?}; skipping"
            ));
            continue;
        };
        if due > now {
            continue;
        }
        let next_due = crate::models::isoformat_utc(
            compute_next_due(&sched.cron, now, &sched.tz)
                .map_err(|error| StorageError::Other(error.to_string()))?,
        );
        if sched.overlap_policy == "skip" && prev_instance_live(store, &sched).await {
            if advance_due_without_work(store, &schedule_id, &occurrence_at, &next_due).await? {
                log(&format!(
                    "schedule {schedule_id}: skip fire (prior job {} still live)",
                    sched.last_job_id
                ));
            }
            continue;
        }
        let Some(claimed) = reserve_due_occurrence(
            store,
            &schedule_id,
            &occurrence_at,
            &next_due,
            &owner,
        )
        .await?
        else {
            log(&format!(
                "schedule {schedule_id}: lost occurrence reservation race"
            ));
            continue;
        };
        match enqueue_pending_occurrence(store, claimed, &owner, now).await {
            Ok(Some(job)) => {
                fired += 1;
                log(&format!(
                    "schedule {schedule_id}: fired job {} (run {}); next_due={next_due}",
                    job.job_id, job.run_id
                ));
            }
            Ok(None) => log(&format!(
                "schedule {schedule_id}: lost occurrence ownership before enqueue"
            )),
            Err(error) => log(&format!(
                "schedule {schedule_id}: occurrence enqueue failed and remains recoverable: {error}"
            )),
        }
    }
    Ok(fired)
}

/// Manually fire a schedule through the same durable occurrence reservation as
/// the coordinator. A pending crash recovery is completed before a new manual
/// occurrence can be created.
pub async fn fire_schedule_now(
    store: &JobStorage,
    schedule_id: &str,
    retry_token: &str,
    now: DateTime<Utc>,
) -> Result<Option<crate::models::Job>, StorageError> {
    let token = format!("{schedule_id}\0{retry_token}");
    let run_id = stable_run_id("schedule", &token);
    if let Some(manifest) = crate::queue::runs::read_run(store, &run_id).await? {
        crate::queue::submit::validate_stored_run_manifest(
            &serde_json::Value::Object(manifest.clone()),
            &run_id,
        )
        .map_err(|error| StorageError::Other(error.to_string()))?;
        if let Some(job) = manifest
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .and_then(|entries| entries.first())
            .and_then(|entry| {
                entry
                    .get("outcome")
                    .and_then(|outcome| outcome.get("job"))
                    .or_else(|| entry.get("planned_job"))
            })
            .cloned()
        {
            return serde_json::from_value(job).map(Some).map_err(StorageError::Json);
        }
    }
    let Some(sched) = read_schedule(store, schedule_id).await? else {
        return Ok(None);
    };
    if sched.deleted && sched.pending_occurrence.is_none() {
        return Ok(None);
    }
    let owner = uuid::Uuid::new_v4().simple().to_string();
    let claimed = if sched.pending_occurrence.is_some() {
        takeover_pending_occurrence(store, schedule_id, &owner).await?
    } else {
        reserve_manual_occurrence(store, schedule_id, &owner, retry_token).await?
    };
    let Some(claimed) = claimed else {
        return Ok(None);
    };
    enqueue_pending_occurrence(store, claimed, &owner, now).await
}
