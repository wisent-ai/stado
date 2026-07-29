//! Version-drift check used by the agent main loop to self-recycle
//! when a newer stado release ships.
//!
//! Port of `stado/providers/local/version_check.py`, with the phase-4
//! DEVIATION below.
//!
//! A running agent compares its compiled version only with the operator-pinned
//! [`crate::config::stado_release_version`]. No channel pointer, package index,
//! or public "latest" resolver is compiled into production builds.
//!
//! DEVIATION (intended): the Rust binary is an immutable Stado artifact:
//!   * drift detection compares `CARGO_PKG_VERSION` with the operator-pinned
//!     release coordinate;
//!   * remediation on a local-kind agent downloads the exact configured
//!     version and platform through the public Stado release object route,
//!     verifies every binary against SHA256SUMS, atomically replaces the
//!     running binary and installed siblings, then re-execs. Any failure is
//!     logged and the old binary continues running;
//!   * cloud agents exit after recording a provider-neutral termination
//!     intent, and the coordinator reaps and replaces them through the owning
//!     provider adapter.
//!   * `WC_SKIP_VERSION_CHECK=1` still short-circuits the whole check —
//!     detection AND remediation;
//!   * the cloud-agent terminate-on-drift path exits after recording a
//!     provider-neutral intent through [`super::self_terminate`]; the
//!     coordinator reaps the machine through its owning provider adapter and
//!     dispatches a replacement with the configured exact release.

#[cfg(test)]
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const IMPORT_OK_TTL: Duration = Duration::from_secs(300);
const IMPORT_BAD_TTL: Duration = Duration::from_secs(30);
#[cfg(test)]
const CACHE_TTL: Duration = IMPORT_BAD_TTL;

/// Version ordering lives in [`crate::release`], which owns every rule about
/// release versions. Re-exported here because this module's PyPI checks and the
/// callers that grew up around them compare versions too, and two
/// implementations of "which of these is newer" is exactly one too many.
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

static IMPORT_CACHE: LazyLock<Mutex<Option<(Instant, bool, String)>>> =
    LazyLock::new(|| Mutex::new(None));

/// Smoke-test `import wisent` in a subprocess of the agent's job-runtime
/// Python. Returns (ok, error_message). Python `wisent_import_ok`.
///
/// Run before claiming a job so a corrupt or incompatible immutable runtime
/// bundle triggers remediation rather than claiming jobs that will fail their
/// first `python -m wisent...` line.
///
/// Deviation: Python uses `sys.executable`; the Rust binary is not a
/// Python interpreter, so it invokes `python3` from the agent's PATH (the
/// agent runs inside its venv, whose bin/ is first on PATH).
pub async fn wisent_import_ok() -> (bool, String) {
    {
        let cache = IMPORT_CACHE.lock().expect("import cache lock");
        if let Some((ts, ok, err)) = &*cache {
            let ttl = if *ok { IMPORT_OK_TTL } else { IMPORT_BAD_TTL };
            if ts.elapsed() < ttl {
                return (*ok, err.clone());
            }
        }
    }
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(super::python_bin())
            .args(["-c", "import wisent"])
            .output(),
    )
    .await;
    let (ok, err) = match result {
        Err(_) => (false, "import smoke test timed out".to_string()),
        Ok(Err(err)) => (false, format!("import smoke test failed to spawn: {err}")),
        Ok(Ok(out)) if out.status.success() => (true, String::new()),
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            // Python: (res.stderr or res.stdout or "(no output)").strip()[:400]
            let raw = if !stderr.is_empty() {
                stderr.into_owned()
            } else if !stdout.is_empty() {
                stdout.into_owned()
            } else {
                "(no output)".to_string()
            };
            (false, raw.trim().chars().take(400).collect())
        }
    };
    *IMPORT_CACHE.lock().expect("import cache lock") = Some((Instant::now(), ok, err.clone()));
    (ok, err)
}

/// What the agent loop should do after [`maybe_drain_or_upgrade`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftOutcome {
    /// No remediation needed (Python returns False).
    Clean,
    /// Skip the claim path this tick (Python returns True -> `continue`).
    SkipClaim,
    /// Drift remediation on a local-kind agent FAILED (self-update error
    /// or re-exec failure) or the venv is broken: the error was logged and
    /// the agent keeps running the current binary. A successful
    /// self-update never produces this outcome — the process re-execs
    /// instead ([`crate::self_update::reexec`]).
    DriftDetected,
    /// Cloud agent: self-terminate was invoked (Python raises
    /// SystemExit(0) right after); the caller should exit(0).
    SelfTerminated,
}

/// Combined drift + venv-integrity handling for the agent main loop.
/// Python `maybe_drain_or_upgrade(slots, log_fn, kind)` — `slots_active`
/// stands in for the drained-slots check (`if slots:` / `if not slots:`).
///
///   1. the exact configured release version is strictly newer than the
///      installed version ([`detect_drift`]).
///   2. `import wisent` raises in the selected immutable runtime bundle.
///
/// Cloud agents (kind != "local") DO NOT upgrade in place on drift; they
/// self-terminate so the dispatcher creates a fresh VM with the new
/// version baked in — no in-process file race possible (that race
/// produced the zombie 1778695548-{2,3,5} VMs on 2026-05-13).
///
/// Local agents remediate binary drift with [`crate::self_update`]: download,
/// verify, atomically replace, then re-exec. A broken Python runtime requires
/// selecting a replacement bundle, so that path keeps the log-and-report
/// behavior.
///
/// Caller MUST advance slots BEFORE calling this so a drained slots list
/// triggers the remediation path.
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
    // Active work defers drift handling. Exact configuration resolution is
    // local and does not create a network liveness dependency.
    if slots_active {
        let (ok, err) = wisent_import_ok().await;
        if ok {
            return DriftOutcome::Clean;
        }
        log_fn(&format!("venv broken while slots active: {err}"));
        return DriftOutcome::SkipClaim;
    }
    let drift = detect_drift().await;
    let (ok, err) = wisent_import_ok().await;
    if drift.is_none() && ok {
        return DriftOutcome::Clean;
    }
    // Slots are drained past this point (Python `if not slots:`).
    if crate::capabilities::execution_adapter(kind)
        != Some(crate::capabilities::ExecutionAdapter::Local)
    {
        log_fn(&format!(
            "cloud agent {kind} drift={drift:?} ok={ok}; self-terminate \
             so dispatcher creates a fresh VM with new version baked in"
        ));
        super::self_terminate(kind, log_fn).await;
        return DriftOutcome::SelfTerminated;
    }
    if !ok {
        // A broken venv is a Python-environment problem; replacing the
        // Rust binary cannot fix it, so keep the log-and-report stance.
        log_fn(&format!("venv broken: {err}"));
        log_fn(
            "venv remediation requires publishing and selecting a replacement immutable \
             runtime bundle; restart the agent after updating that exact coordinate",
        );
        return DriftOutcome::DriftDetected;
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
                "self-update failed (drift {drift:?}): {update_err}; \
                 keeping the current binary running"
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
