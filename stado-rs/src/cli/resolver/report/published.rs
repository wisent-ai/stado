// ---------------------------------------------------------------------------
// What the resolver publishes about itself
// ---------------------------------------------------------------------------

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Where `serve` publishes what it holds, under `~/.stado`.
///
/// Until 2026-08-19 the directory generation and the reason an upstream read
/// failed lived in this process's memory and in an 83 MiB stderr log, nowhere
/// else. So while this host's resolver sat in a launchd restart loop -- `last
/// exit code = 69: EX_UNAVAILABLE`, restarted on a five second
/// `ThrottleInterval` -- the two questions the operator had, which generation
/// it holds and why it cannot load another, had no answer anywhere in the
/// product. This file is the answer and [`status`] is its reader. It stays
/// readable with the resolver stopped, which is exactly when it gets read.
pub(super) const STATE_FILE: &str = "resolver-state.json";

/// Operator override for [`STATE_FILE`]'s location, absolute.
const STATE_FILE_ENV: &str = "STADO_RESOLVER_STATE_FILE";

/// Serving traffic from a snapshot it holds.
pub(super) const RESOLVER_SERVING: &str = "serving";
/// Reading its first snapshot; no port is bound yet.
const RESOLVER_STARTING: &str = "starting";
/// An upstream read failed and the next attempt is scheduled.
const RESOLVER_BACKING_OFF: &str = "backing_off";
/// Stopped for a reason no retry clears.
const RESOLVER_FAILED: &str = "failed";
/// No state file exists: no resolver has run since one was last removed.
/// Never written, only reported by [`status`].
pub(super) const RESOLVER_UNPUBLISHED: &str = "unpublished";

/// First delay after a failed upstream read.
const BACKOFF_BASE: Duration = Duration::from_secs(1);

/// Ceiling on that delay.
///
/// Bounded in both directions, deliberately. The behaviour this replaces was
/// unbounded upward in restarts and downward in patience: `serve` exited 69,
/// launchd restarted it five seconds later, and the loop neither slowed down
/// nor said why. Retrying in place at a capped interval keeps one pid, one
/// log and one published reason, and a resolver that has been waiting an hour
/// still retries within the minute the authority comes back.
const BACKOFF_CAP: Duration = Duration::from_secs(60);

/// [`BACKOFF_BASE`] doubled per consecutive failure, capped at
/// [`BACKOFF_CAP`].
pub(crate) fn backoff_delay(attempt: u32) -> Duration {
    let doublings = attempt.saturating_sub(1).min(u32::BITS - 1);
    Duration::from_secs(
        BACKOFF_BASE
            .as_secs()
            .checked_shl(doublings)
            .unwrap_or(u64::MAX)
            .min(BACKOFF_CAP.as_secs()),
    )
}

/// `datetime.now(timezone.utc).isoformat()`, as every other writer in the
/// crate stamps it (`queue/leases.rs::now_iso`).
pub(crate) fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// What one `resolver serve` process holds, and why it holds nothing more.
///
/// Read tolerantly (`serde(default)`): a newer resolver writing a field this
/// build does not model must not make [`status`] report a host with no
/// resolver at all, which is the strictness failure
/// `service_resolution::ServiceRoute` records at fleet scale.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PublishedState {
    /// When this file was written.
    pub(super) updated_at: String,
    /// Registry target whose `service_resolver` policy the process enforces.
    pub(super) target: String,
    /// The process that wrote it.
    pub(super) pid: u32,
    /// [`RESOLVER_SERVING`], [`RESOLVER_STARTING`], [`RESOLVER_BACKING_OFF`]
    /// or [`RESOLVER_FAILED`].
    pub(super) state: String,
    /// Directory generation held, absent until one is.
    pub(super) generation: Option<u64>,
    /// Registry store version that generation came from -- the value the
    /// adapter refusal quotes as `store generation 945077b5...`.
    pub(super) store_version: Option<String>,
    /// When that snapshot was loaded.
    pub(super) loaded_at: Option<String>,
    /// Why the process is not serving, in the upstream's own words.
    pub(super) reason: Option<String>,
    /// Consecutive failed upstream reads.
    pub(super) attempt: u32,
    /// When the next upstream read is due.
    pub(super) next_attempt_at: Option<String>,
    /// Why this host last refused to refresh its last-known-good registry
    /// copy ([`targets::LastGoodRefusal::kind`]), absent while the copy is
    /// being kept current.
    ///
    /// The slug only: the underlying sentence names a path and the
    /// authority's own words, and this file is read by another process.
    pub(super) last_good_refusal: Option<String>,
}

impl PublishedState {
    pub(crate) fn starting(target: &str) -> Self {
        Self {
            updated_at: now_iso(),
            target: target.to_string(),
            pid: std::process::id(),
            state: RESOLVER_STARTING.to_string(),
            ..Self::default()
        }
    }

    pub(crate) fn serving(
        target: &str,
        generation: u64,
        store_version: &str,
        loaded_at: &str,
        last_good_refusal: Option<&str>,
    ) -> Self {
        Self {
            updated_at: now_iso(),
            target: target.to_string(),
            pid: std::process::id(),
            state: RESOLVER_SERVING.to_string(),
            generation: Some(generation),
            store_version: Some(store_version.to_string()),
            loaded_at: Some(loaded_at.to_string()),
            last_good_refusal: last_good_refusal.map(str::to_string),
            ..Self::default()
        }
    }

    pub(crate) fn backing_off(target: &str, attempt: u32, reason: &str, delay: Duration) -> Self {
        let ahead = chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::zero());
        Self {
            updated_at: now_iso(),
            target: target.to_string(),
            pid: std::process::id(),
            state: RESOLVER_BACKING_OFF.to_string(),
            reason: Some(reason.to_string()),
            attempt,
            next_attempt_at: Some((chrono::Utc::now() + ahead).to_rfc3339()),
            ..Self::default()
        }
    }

    pub(crate) fn failed(target: &str, reason: &str) -> Self {
        Self {
            updated_at: now_iso(),
            target: target.to_string(),
            pid: std::process::id(),
            state: RESOLVER_FAILED.to_string(),
            reason: Some(reason.to_string()),
            ..Self::default()
        }
    }
}

pub(super) fn state_path() -> Option<std::path::PathBuf> {
    if let Some(explicit) = std::env::var_os(STATE_FILE_ENV) {
        let path = std::path::PathBuf::from(explicit);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|home| home.join(".stado").join(STATE_FILE))
}

/// Publish the state, atomically, best effort.
///
/// Best effort on purpose: a resolver that cannot write its own diagnostic
/// file is still one that can serve traffic, and refusing to start over it
/// would make this file the outage. Written to a sibling and renamed rather
/// than in place, because [`status`] reads it while `serve` writes it and a
/// half-written document reads as a resolver that has never run.
pub(crate) fn publish(state: &PublishedState) {
    let Some(path) = state_path() else { return };
    let Some(parent) = path.parent() else { return };
    let body = match serde_json::to_vec_pretty(state) {
        Ok(body) => body,
        Err(error) => {
            eprintln!("stado resolver could not serialize its state: {error}");
            return;
        }
    };
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let written = std::fs::create_dir_all(parent)
        .and_then(|()| std::fs::write(&temp, &body))
        .and_then(|()| std::fs::rename(&temp, &path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temp);
        eprintln!(
            "stado resolver could not publish its state to {}: {error}",
            path.display()
        );
    }
}

/// The state the last `serve` process published, when there is one.
pub(super) fn published_state() -> Option<PublishedState> {
    let body = std::fs::read_to_string(state_path()?).ok()?;
    serde_json::from_str(&body).ok()
}
