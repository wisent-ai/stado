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

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::{LazyLock, Mutex};
#[cfg(test)]
use std::time::{Duration, Instant};
#[cfg(test)]
const CACHE_TTL: Duration = Duration::from_secs(b'\x1e' as u64);

/// Version ordering lives in [`crate::release`], which owns every rule about
/// release versions. Re-exported here for compatibility tests.
pub use crate::release::{version_newer, version_tuple, VersionToken};

/// Pure: newest release key of a PyPI /pypi/<pkg>/json payload.
/// None when there are no releases (Python `if not releases: return None`).
#[cfg(test)]
pub fn latest_release_from_json(body: &serde_json::Value) -> Option<String> {
    let releases = body.get("releases")?.as_object()?;
    if releases.is_empty() {
        return None;
    }
    releases
        .keys()
        .max_by(|a, b| version_tuple(a).cmp(&version_tuple(b)))
        .cloned()
}

#[cfg(test)]
static PYPI_CACHE: LazyLock<Mutex<HashMap<String, (Instant, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Python `_pypi_latest`: cached legacy helper retained for reference and
/// offline compatibility tests. Drift detection never consults it.
#[cfg(test)]
pub async fn pypi_latest(pkg: &str) -> Option<String> {
    pypi_latest_at(&format!("https://pypi.org/pypi/{pkg}/json"), pkg).await
}

/// [`pypi_latest`] against an explicit URL (tests inject a loopback server
/// or fabricate failures offline).
#[cfg(test)]
pub async fn pypi_latest_at(url: &str, pkg: &str) -> Option<String> {
    let cached = PYPI_CACHE
        .lock()
        .expect("pypi cache lock")
        .get(pkg)
        .cloned();
    if let Some((ts, latest)) = &cached {
        if ts.elapsed() < CACHE_TTL {
            return Some(latest.clone());
        }
    }
    let body = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .and_then(|resp| resp.error_for_status())
        .ok();
    let latest = match body {
        Some(resp) => resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| latest_release_from_json(&v)),
        None => None,
    };
    match latest {
        Some(latest) => {
            PYPI_CACHE
                .lock()
                .expect("pypi cache lock")
                .insert(pkg.to_string(), (Instant::now(), latest.clone()));
            Some(latest)
        }
        // Network/parse failure: stale cache beats nothing (Python
        // `return cached[1] if cached else None`).
        None => cached.map(|(_, latest)| latest),
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_tuple_ordering_matches_python() {
        assert!(version_newer("1.2.9", "1.2.10"));
        assert!(version_newer("0.4.99", "0.4.100"));
        assert!(version_newer("1.0", "1.0.1"));
        assert!(!version_newer("1.0.1", "1.0.1"));
        assert!(!version_newer("1.0.1", "1.0"));
        // Numeric tokens sort before string tokens: 1.0.1 < 1.0a1? No —
        // position 2 is Num(1) vs Str("a1") and Num < Str, so 1.0.1 < 1.0a1.
        assert!(version_newer("1.0.1", "1.0a1"));
        // Hyphens split like dots (Python replace("-", ".")); a suffix
        // EXTENDS the tuple, and a proper prefix sorts before its
        // extension: 1.0 < 1.0-rc1 < 1.0.1? No — tuple compare stops at
        // the shorter length only when the prefix is equal, so
        // (1,0) < (1,0,"rc1") and 1.0-rc1 is the NEWER one.
        assert!(version_newer("1.0", "1.0-rc1"));
        // rc tokens compare as strings after numerics.
        assert!(version_newer("1.0rc1", "1.0rc2"));
    }

    #[test]
    fn latest_release_picks_version_max() {
        let body = serde_json::json!({
            "releases": {"0.4.9": [], "0.4.100": [], "0.4.99": [], "0.3.0": []}
        });
        assert_eq!(latest_release_from_json(&body).as_deref(), Some("0.4.100"));
        assert_eq!(
            latest_release_from_json(&serde_json::json!({"releases": {}})),
            None
        );
        assert_eq!(latest_release_from_json(&serde_json::json!({})), None);
    }

    #[tokio::test]
    async fn pypi_latest_falls_back_to_cache_on_failure() {
        // Seed the cache via a successful loopback fetch, then break the
        // server and confirm the stale value is returned after TTL expiry.
        let pkg = "stado-test-cache-pkg";
        let server = crate::testutil::mock_http(vec![crate::testutil::http_response(
            200,
            "OK",
            r#"{"releases": {"0.1.0": [], "0.2.0": []}}"#,
        )])
        .await;
        let url = format!("{}/pypi/{}/json", server.base_url, pkg);
        assert_eq!(pypi_latest_at(&url, pkg).await.as_deref(), Some("0.2.0"));
        server.stop();

        // Force expiry by rewinding the cache entry past the TTL, then
        // fetch from the dead server: the cached value must come back.
        PYPI_CACHE.lock().unwrap().insert(
            pkg.to_string(),
            (
                Instant::now() - CACHE_TTL - Duration::from_secs(1),
                "0.2.0".to_string(),
            ),
        );
        assert_eq!(pypi_latest_at(&url, pkg).await.as_deref(), Some("0.2.0"));
        // Cleanup so no other test observes the seeded entry.
        PYPI_CACHE.lock().unwrap().remove(pkg);
    }
}
