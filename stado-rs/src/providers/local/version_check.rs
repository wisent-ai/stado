//! Version-drift check used by the agent main loop to self-recycle
//! when a newer stado release ships.
//!
//! A running agent compares its compiled version only with the
//! operator-pinned [`crate::config::stado_release_version`]. No mutable
//! channel pointer, package index, or public "latest" resolver is compiled
//! into production builds.
//!
//! Local remediation downloads the exact configured version and platform,
//! verifies the immutable manifest and checksums, atomically replaces the
//! installed binaries, and re-execs. Failure leaves the current binary
//! running. Cloud machines record a provider-neutral termination intent and
//! are replaced through their owning adapter. `WC_SKIP_VERSION_CHECK` remains
//! the explicit operator escape hatch for detection and remediation.

/// Version ordering lives in [`crate::release`], which owns every rule about
/// release versions. Re-exported here for compatibility tests.
pub use crate::release::{version_newer, version_tuple, VersionToken};

/// Pure: newest release key of a PyPI /pypi/<pkg>/json payload.
/// None when there are no releases (Python `if not releases: return None`).
/// `(installed, desired)` when the exact configured Stado release version is
/// strictly newer than this binary; `None` when the coordinate is absent,
/// malformed, or not newer.
pub async fn detect_drift() -> Option<(String, String)> {
    let installed = env!("CARGO_PKG_VERSION").to_string();
    let desired = crate::config::stado_release_version();
    let canonical = !desired.is_empty()
        && desired.trim() == desired
        && desired
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    (canonical && version_newer(&installed, &desired)).then_some((installed, desired))
}

/// What the agent loop should do after [`maybe_drain_or_upgrade`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftOutcome {
    /// No remediation needed.
    Clean,
    /// Drift remediation on a local-kind agent failed. The error was logged
    /// and the agent keeps running the current binary. A successful update
    /// re-execs instead ([`crate::self_update::reexec`]).
    DriftDetected,
    /// Cloud replacement was requested; the caller should exit.
    SelfTerminated,
}

/// Handle immutable-binary release drift for the agent main loop.
///
/// Workload runtime checks belong to the submitted job and its declared
/// requirements. The Stado agent itself does not import Python packages before
/// claiming unrelated shell, native, or container workloads.
///
/// Cloud agents do not upgrade in place; they self-terminate so the dispatcher
/// creates a fresh machine with the configured immutable release.
pub async fn maybe_drain_or_upgrade(
    slots_active: bool,
    log_fn: &mut dyn FnMut(&str),
    kind: &str,
) -> DriftOutcome {
    if std::env::var("WC_SKIP_VERSION_CHECK")
        .map(|v| v.trim() == "1")
        .unwrap_or(false)
    {
        return DriftOutcome::Clean;
    }
    // Active work defers release replacement. Runtime checks are job-scoped
    // and therefore do not block the global claim loop.
    if slots_active {
        return DriftOutcome::Clean;
    }
    let Some(drift) = detect_drift().await else {
        return DriftOutcome::Clean;
    };
    if crate::capabilities::execution_adapter(kind)
        != Some(crate::capabilities::ExecutionAdapter::Local)
    {
        log_fn(&format!(
            "cloud agent {kind} release drift={drift:?}; self-terminate \
             so dispatcher creates a fresh machine with the configured release"
        ));
        super::self_terminate(kind, log_fn).await;
        return DriftOutcome::SelfTerminated;
    }
    // Binary self-update downloads the exact configured release, verifies it,
    // atomically replaces the executable, then re-execs.
    match crate::self_update::self_update(log_fn).await {
        Ok(crate::self_update::UpdateOutcome::Updated { from, to }) => {
            log_fn(&format!(
                "self-update {from} -> {to} installed; re-executing the new binary"
            ));
            let exec_err = crate::self_update::reexec();
            log_fn(&format!(
                "re-exec after self-update failed: {exec_err}; continuing on the old in-memory binary"
            ));
        }
        Ok(crate::self_update::UpdateOutcome::UpToDate { .. }) => {
            log_fn("configured release is no longer newer than the installed binary");
            return DriftOutcome::Clean;
        }
        Err(update_err) => {
            log_fn(&format!(
                "self-update failed (drift {drift:?}): {update_err}; keeping the current binary running"
            ));
        }
    }
    DriftOutcome::DriftDetected
}
