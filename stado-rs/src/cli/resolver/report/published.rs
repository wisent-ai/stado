// ---------------------------------------------------------------------------
// What the resolver publishes about itself
// ---------------------------------------------------------------------------

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
    pub(super) pid: u32,
    /// [`RESOLVER_SERVING`], [`RESOLVER_STARTING`], [`RESOLVER_BACKING_OFF`]
    /// or [`RESOLVER_FAILED`].
    pub(super) state: String,
    /// Directory generation held, absent until one is.
    pub(super) generation: Option<u64>,
    /// Registry store version that generation came from -- the value the
    /// adapter refusal quotes as `store generation <version>`.
    pub(super) store_version: Option<String>,
    /// When that snapshot was loaded.
    pub(super) loaded_at: Option<String>,
    /// Why the process is not serving, in the upstream's own words.
    pub(super) reason: Option<String>,
    /// Consecutive failed upstream reads.
    pub(super) attempt: u32,
    /// When the next upstream read is due, on the declared refresh interval.
    pub(super) next_attempt_at: Option<String>,
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
    /// keeping a copy of the port in its own configuration.
    pub(super) adapters: Vec<PublishedAdapter>,
}

/// One adapter's listening address, for one service and one consumer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PublishedAdapter {
    pub(crate) service: String,
    pub(crate) consumer: String,
    pub(crate) bind: String,
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
        let ahead =
            chrono::Duration::from_std(refresh).unwrap_or_else(|_| chrono::Duration::zero());
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

    /// The same state, written by a process whose listeners are bound at
    /// `adapters`.
    pub(crate) fn bound(mut self, adapters: Vec<PublishedAdapter>) -> Self {
        self.listening = true;
        self.adapters = adapters;
        self
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
        eprintln!(
            "{}",
            crate::primitives::staging::failed_with(
                format!("stado resolver could not publish its state to {}: {error}", path.display()),
                &temp,
            )
        );
    }
}

/// The state the last `serve` process published, when there is one. A state
/// file that exists but cannot be read or parsed is named on stderr with its
/// cause: answered as none, status reported that no resolver had published
/// while one had, and every adapter lookup through it looked unserved.
pub(super) fn published_state() -> Option<PublishedState> {
    let path = state_path()?;
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!(
                "stado resolver state {} cannot be read: {error}",
                path.display()
            );
            return None;
        }
    };
    match serde_json::from_str(&body) {
        Ok(state) => Some(state),
        Err(error) => {
            eprintln!(
                "stado resolver state {} is not a published state this build reads: {error}",
                path.display()
            );
            None
        }
    }
}

/// `http://<bind>` of this host's adapter for `service` and `consumer`, as
/// the resolver holding its listeners bound published it; `None` while no
/// such resolver has published that adapter.
pub(crate) fn published_adapter_url(service: &str, consumer: &str) -> Option<String> {
    let state = published_state()?;
    if !state.listening {
        return None;
    }
    state
        .adapters
        .into_iter()
        .find(|adapter| adapter.service == service && adapter.consumer == consumer)
        .map(|adapter| format!("http://{}", adapter.bind))
}

/// Why this host's resolver holds no listener, in the words it published:
/// the state, the process that wrote it, when, and its reason. `None` while
/// no resolver has published or the one that did holds its listeners bound.
///
/// A connect refused on a resolver adapter's loopback bind reads as the
/// placed service failing to answer, while the socket nobody opened belongs
/// to a resolver that stopped for a reason it wrote down here; naming that
/// reason beside the refusal is what makes the outage readable where it is
/// met. A state published without a reason is said as such, not invented.
pub(crate) fn not_serving_reason() -> Option<String> {
    let state = published_state()?;
    if state.listening {
        return None;
    }
    let written = format!(
        "this host's resolver (pid {}, state {} at {}) holds no listener",
        state.pid, state.state, state.updated_at
    );
    Some(
        match state.reason.filter(|reason| !reason.trim().is_empty()) {
            Some(reason) => format!("{written}: {reason}"),
            None => format!("{written}, and published no reason for it"),
        },
    )
}

/// `STADO_RESOLVER_STATE`, the published state, the pid that wrote it, when
/// (epoch) and whether that pid holds its listeners bound (`1`/`0`), for the
/// host-side question whether a resolver role took over its old unit's ports;
/// `None` when no resolver has published.
pub(crate) fn readiness_marker() -> Option<String> {
    let state = published_state()?;
    let written = chrono::DateTime::parse_from_rfc3339(&state.updated_at).ok()?;
    Some(format!(
        "STADO_RESOLVER_STATE\t{}\t{}\t{}\t{}",
        state.state,
        state.pid,
        written.timestamp(),
        u8::from(state.listening)
    ))
}
