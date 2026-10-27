// ---------------------------------------------------------------------------
// What the resolver publishes about itself
// ---------------------------------------------------------------------------

mod file;
mod lifetime;

pub(crate) use file::{publish, publisher_running};
pub(in crate::cli::resolver) use file::{published_state, state_path};

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Where `serve` publishes what it holds, under `~/.stado`.
///
/// The directory generation the resolver holds and the reason an upstream
/// read failed must live somewhere other than this process's memory and its
/// stderr log: while a resolver sits in a launchd restart loop, the two
/// questions an operator has -- which generation it holds and why it cannot
/// load another -- need an answer in the product. This file is the answer
/// and [`status`] is its reader. It stays readable with the resolver
/// stopped, which is exactly when it gets read.
pub(super) const STATE_FILE: &str = "resolver-state.json";

/// Operator override for [`STATE_FILE`]'s location, absolute.
const STATE_FILE_ENV: &str = "STADO_RESOLVER_STATE_FILE";

/// Serving traffic from a snapshot it holds.
pub(super) const RESOLVER_SERVING: &str = "serving";
/// Reading its first snapshot; no port is bound yet.
const RESOLVER_STARTING: &str = "starting";
/// An upstream read failed; the next one is due on the declared refresh
/// interval.
const RESOLVER_BACKING_OFF: &str = "backing_off";
/// Stopped for a reason no retry clears.
const RESOLVER_FAILED: &str = "failed";
/// No state file exists: no resolver has run since one was last removed.
/// Never written, only reported by [`status`].
pub(super) const RESOLVER_UNPUBLISHED: &str = "unpublished";

/// `datetime.now(timezone.utc).isoformat()`, as every other writer in the
/// crate stamps it (`queue/leases.rs::now_iso`).
pub(crate) fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// What one `serve --resolver` role holds, and why it holds nothing more.
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
    pub(in crate::cli::resolver) pid: u32,
    /// [`RESOLVER_SERVING`], [`RESOLVER_STARTING`], [`RESOLVER_BACKING_OFF`]
    /// or [`RESOLVER_FAILED`].
    pub(super) state: String,
    /// Directory generation held, absent until one is.
    pub(super) generation: Option<u64>,
    /// Registry store version that generation came from -- the value the
    /// adapter refusal quotes as `store generation <version>`.
    pub(super) store_version: Option<String>,
    /// The registry authority that generation was read from; absent when it
    /// was read from this host's own registry store. A refused rollback
    /// names this source, so a reader can tell which host delivered it.
    pub(super) read_from: Option<String>,
    /// When that snapshot was loaded.
    pub(super) loaded_at: Option<String>,
    /// Why the process is not serving, in the upstream's own words.
    pub(super) reason: Option<String>,
    /// Consecutive failed upstream reads.
    pub(super) attempt: u32,
    /// When the next upstream read is due, on the declared refresh interval.
    pub(super) next_attempt_at: Option<String>,
    /// The refresh interval this process runs on, as the registry declared
    /// it: the measure every reader on this host holds a wait against
    /// (`waiting::held_at`, `waiting::inflight`), published by the process
    /// whose waits they are. Absent before the first registry read names it.
    pub(super) refresh_seconds: Option<u64>,
    /// Why this host last refused to refresh its last-known-good registry
    /// copy ([`targets::LastGoodRefusal::kind`]), absent while the copy is
    /// being kept current.
    ///
    /// The slug only: the underlying sentence names a path and the
    /// authority's own words, and this file is read by another process.
    pub(super) last_good_refusal: Option<String>,
    /// Whether the process that wrote this holds its listeners bound. Kept
    /// apart from `state`, which is the registry's health: a failed refresh
    /// publishes `backing_off` while every port stays bound, and a reader
    /// asking who owns the ports must not read that as "not listening".
    pub(super) listening: bool,
    /// The address each adapter listens on, as this process bound it. A
    /// program on this host reaches a resolved service here instead of
    /// keeping a copy of the port in its own configuration. A declared
    /// adapter this process does not serve is listed with its reason.
    pub(super) adapters: Vec<PublishedAdapter>,
    /// Where this process answers its resolution API, so `stado resolver
    /// snapshot` asks it for the generation it holds in memory instead of
    /// reading the registry store, which can stand.
    pub(in crate::cli::resolver) api_bind: Option<String>,
}

/// One adapter's listening address, for one service and one consumer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PublishedAdapter {
    pub(crate) service: String,
    pub(crate) consumer: String,
    pub(crate) bind: String,
    /// Why this process does not serve the adapter; absent when it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unserved: Option<String>,
}

impl PublishedState {
    /// Published before the first registry read; `reason` says what that
    /// read waits on, so a reader of this file while no port is bound is
    /// told what the process is doing instead of "published no reason".
    pub(crate) fn starting(target: &str, reason: &str) -> Self {
        Self {
            updated_at: now_iso(),
            target: target.to_string(),
            pid: std::process::id(),
            state: RESOLVER_STARTING.to_string(),
            reason: Some(reason.to_string()),
            ..Self::default()
        }
    }

    pub(crate) fn serving(
        target: &str,
        generation: u64,
        store_version: &str,
        loaded_at: &str,
        last_good_refusal: Option<&str>,
        adapters: Vec<PublishedAdapter>,
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
            listening: true,
            adapters,
            ..Self::default()
        }
    }

    pub(crate) fn backing_off(target: &str, attempt: u32, reason: &str, refresh: Duration) -> Self {
        let due = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now() + refresh);
        Self {
            updated_at: now_iso(),
            target: target.to_string(),
            pid: std::process::id(),
            state: RESOLVER_BACKING_OFF.to_string(),
            reason: Some(reason.to_string()),
            attempt,
            next_attempt_at: Some(due.to_rfc3339()),
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

    /// The same state, written by a process whose listeners are bound at
    /// `adapters`.
    pub(crate) fn bound(mut self, adapters: Vec<PublishedAdapter>) -> Self {
        self.listening = true;
        self.adapters = adapters;
        self
    }

    /// The same state, written by a process that refreshes on `seconds`.
    pub(crate) fn refreshing(mut self, seconds: u64) -> Self {
        self.refresh_seconds = Some(seconds);
        self
    }

    /// The same state, written by a process answering its API at `bind`.
    pub(crate) fn answering_at(mut self, bind: &str) -> Self {
        self.api_bind = Some(bind.to_string());
        self
    }

    /// The same state, holding a generation read from `authority`'s
    /// snapshot, or from this host's own registry store when `None`.
    pub(crate) fn read_from(mut self, authority: Option<String>) -> Self {
        self.read_from = authority;
        self
    }
}
