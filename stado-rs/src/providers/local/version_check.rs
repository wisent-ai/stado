//! Version-drift check used by the agent main loop to self-recycle
//! when a newer stado release ships.
//!
//! Port of `stado/providers/local/version_check.py`, with the phase-4
//! DEVIATION below.
//!
//! Without this, an agent installed at boot time keeps running its
//! original version forever — newer releases never reach the running fleet
//! until GCE preempts the VM, idle_shutdown fires (which only triggers
//! when the queue stops yielding work), or an operator manually deletes
//! the VM. The class of bugs this surfaces is "fixes shipped but not
//! running": e.g. wisent-compute 0.4.59's verify_command hook, wisent
//! 0.11.23's batched HF commits, wisent-tools 0.1.16's
//! exit-non-zero-on-strategy-failure — all on PyPI but ignored by 32
//! already-running pre-fix VMs on 2026-05-06 producing fake-COMPLETED jobs
//! at ~80/hour.
//!
//! DEVIATION (intended): the Rust binary is not pip-installed, so
//!   * drift DETECTION compares the crate version (CARGO_PKG_VERSION)
//!     against `gs://wisent-compute/releases/stado/latest.json` — the
//!     release channel the Rust binaries actually ship through (see
//!     [`crate::self_update`]). PyPI is deliberately NOT consulted: the
//!     `stado` PyPI package tracks the Python implementation, whose
//!     version numbers no longer describe the running Rust binary. The
//!     PyPI helpers below ([`pypi_latest`], [`latest_release_from_json`])
//!     are retained for reference and their tests but no longer feed
//!     detection. `wisent` / `wisent-tools` drift of the agent's Python
//!     venv is NOT tracked here either (the venv smoke test below still
//!     covers a broken venv);
//!   * remediation on a local-kind agent IS ported as binary self-update
//!     (Python's `pip_upgrade_and_exec`: `pip install --upgrade stado` +
//!     `os.execv`): [`crate::self_update::self_update`] downloads the new
//!     release from `gs://wisent-compute/releases/stado/<version>/<platform>/`,
//!     verifies every binary against SHA256SUMS, atomically replaces the
//!     running binary + same-dir siblings, then [`crate::self_update::reexec`]
//!     replaces the process image. On ANY failure the error is logged and
//!     the agent keeps running the old binary
//!     ([`DriftOutcome::DriftDetected`]);
//!   * `WC_SKIP_VERSION_CHECK=1` still short-circuits the whole check —
//!     detection AND remediation;
//!   * the cloud-agent self-terminate-on-drift path IS kept (calls
//!     [`super::gcp_self::self_terminate`]); the dispatcher creates a fresh
//!     VM whose startup installs the new version before the agent starts.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Legacy PyPI-era package list (Python `_PACKAGES`, reduced to the
/// binary itself). Retained for the PyPI helpers below; drift DETECTION
/// no longer consults PyPI (see the module-docs deviation).
pub const PACKAGES: [&str; 1] = ["stado"];

const IMPORT_OK_TTL: Duration = Duration::from_secs(300);
const IMPORT_BAD_TTL: Duration = Duration::from_secs(30);
// In-process cache TTL for the retained PyPI helper. (When PyPI fed
// drift detection this was lowered from 300s so a fresh release reached
// agents within one loop iteration; detection now reads GCS latest.json
// per tick, so this only throttles the legacy helper.)
const CACHE_TTL: Duration = Duration::from_secs(30);

/// One token of Python `_version_tuple`: (0, int) for numeric tokens,
/// (1, str) otherwise — numeric tokens always sort before string tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionToken {
    Num(i64),
    Str(String),
}

impl Ord for VersionToken {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (VersionToken::Num(a), VersionToken::Num(b)) => a.cmp(b),
            (VersionToken::Str(a), VersionToken::Str(b)) => a.cmp(b),
            // Python (0, int) < (1, str).
            (VersionToken::Num(_), VersionToken::Str(_)) => Ordering::Less,
            (VersionToken::Str(_), VersionToken::Num(_)) => Ordering::Greater,
        }
    }
}

impl PartialOrd for VersionToken {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Python `_version_tuple`: split on "." and "-", numeric tokens as ints.
/// Slice comparison matches Python tuple semantics (a proper prefix sorts
/// before its extension: 1.0 < 1.0.1).
pub fn version_tuple(v: &str) -> Vec<VersionToken> {
    v.replace('-', ".")
        .split('.')
        .map(|token| match token.parse::<i64>() {
            Ok(n) => VersionToken::Num(n),
            // i64 overflow lands here too; real release versions never do.
            Err(_) => VersionToken::Str(token.to_string()),
        })
        .collect()
}

/// True when `latest` is strictly newer than `installed`.
pub fn version_newer(installed: &str, latest: &str) -> bool {
    version_tuple(installed) < version_tuple(latest)
}

/// Pure: newest release key of a PyPI /pypi/<pkg>/json payload.
/// None when there are no releases (Python `if not releases: return None`).
pub fn latest_release_from_json(body: &serde_json::Value) -> Option<String> {
    let releases = body.get("releases")?.as_object()?;
    if releases.is_empty() {
        return None;
    }
    releases.keys().max_by(|a, b| version_tuple(a).cmp(&version_tuple(b))).cloned()
}

static PYPI_CACHE: LazyLock<Mutex<HashMap<String, (Instant, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Python `_pypi_latest`: 30s in-process cache; network/JSON failures fall
/// back to the cached value (or None). LEGACY: no longer consulted for
/// drift detection (that reads GCS latest.json now); retained for
/// reference and its tests.
pub async fn pypi_latest(pkg: &str) -> Option<String> {
    pypi_latest_at(&format!("https://pypi.org/pypi/{pkg}/json"), pkg).await
}

/// [`pypi_latest`] against an explicit URL (tests inject a loopback server
/// or fabricate failures offline).
pub async fn pypi_latest_at(url: &str, pkg: &str) -> Option<String> {
    let cached = PYPI_CACHE.lock().expect("pypi cache lock").get(pkg).cloned();
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
        Some(resp) => resp.json::<serde_json::Value>().await.ok().and_then(|v| latest_release_from_json(&v)),
        None => None,
    };
    match latest {
        Some(latest) => {
            PYPI_CACHE.lock().expect("pypi cache lock").insert(pkg.to_string(), (Instant::now(), latest.clone()));
            Some(latest)
        }
        // Network/parse failure: stale cache beats nothing (Python
        // `return cached[1] if cached else None`).
        None => cached.map(|(_, latest)| latest),
    }
}

/// (installed, latest) when the release channel's latest version is
/// strictly newer than the installed version; None = no drift. Python
/// `detect_drift`, with the detection source switched from PyPI to
/// `gs://wisent-compute/releases/stado/latest.json` (see the module-docs
/// deviation — PyPI tracks the Python package, not these binaries).
pub async fn detect_drift() -> Option<(String, String)> {
    let installed = env!("CARGO_PKG_VERSION").to_string();
    let latest = crate::self_update::check_latest().await?.version;
    version_newer(&installed, &latest).then_some((installed, latest))
}

static IMPORT_CACHE: LazyLock<Mutex<Option<(Instant, bool, String)>>> =
    LazyLock::new(|| Mutex::new(None));

/// Smoke-test `import wisent` in a subprocess of the agent's job-runtime
/// Python. Returns (ok, error_message). Python `wisent_import_ok`.
///
/// Run before claiming a job so a broken venv (e.g. PyPI ships a wheel
/// whose __init__.py imports a name that the same release forgot to
/// re-export, as wisent 0.11.36 did with ImageAdapter) triggers
/// remediation rather than claiming jobs that will fail their first
/// `python -m wisent...` line.
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
/// Two triggers for remediation:
///   1. `gs://wisent-compute/releases/stado/latest.json` naming a release
///      strictly newer than the installed version ([`detect_drift`]).
///   2. `import wisent` raises in the venv (broken wheel published to
///      PyPI — happened with wisent 0.11.36's missing ImageAdapter
///      re-export; agents booted at exactly the wrong moment cached the
///      bad install and accepted+failed 5+ jobs in a row before this
///      check existed).
///
/// Cloud agents (kind != "local") DO NOT upgrade in place on drift; they
/// self-terminate so the dispatcher creates a fresh VM with the new
/// version baked in — no in-process file race possible (that race
/// produced the zombie 1778695548-{2,3,5} VMs on 2026-05-13).
///
/// Local agents remediate drift with [`crate::self_update`]: download +
/// verify + atomically replace, then re-exec (Python's
/// pip_upgrade_and_exec semantics). A broken venv is NOT fixable by
/// replacing the Rust binary, so that path keeps the old log-and-report
/// behavior.
///
/// Caller MUST advance slots BEFORE calling this so a drained slots list
/// triggers the remediation path.
pub async fn maybe_drain_or_upgrade(
    slots_active: bool,
    log_fn: &mut dyn FnMut(&str),
    kind: &str,
) -> DriftOutcome {
    if std::env::var("WC_SKIP_VERSION_CHECK").map(|v| v.trim() == "1").unwrap_or(false) {
        return DriftOutcome::Clean;
    }
    // If a job is active, drift cannot be applied yet. Avoid making the
    // release bucket a liveness dependency for running work; a transient
    // GCS reset must not raise out of detect_drift() and crash the agent
    // while a slot is active.
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
    if kind != "local" {
        log_fn(&format!(
            "cloud agent {kind} drift={drift:?} ok={ok}; self-terminate \
             so dispatcher creates a fresh VM with new version baked in"
        ));
        super::gcp_self::self_terminate(log_fn).await;
        return DriftOutcome::SelfTerminated;
    }
    if !ok {
        // A broken venv is a Python-environment problem; replacing the
        // Rust binary cannot fix it, so keep the log-and-report stance.
        log_fn(&format!("venv broken: {err}"));
        log_fn(
            "venv remediation needs the Python environment; the Rust agent \
             cannot pip-install itself — repair the venv and restart the agent",
        );
        return DriftOutcome::DriftDetected;
    }
    // Binary self-update (Python pip_upgrade_and_exec: `pip install
    // --upgrade stado` + os.execv): download + verify + atomically
    // replace, then re-exec. On success reexec never returns.
    match crate::self_update::self_update(log_fn).await {
        Ok(crate::self_update::UpdateOutcome::Updated { from, to }) => {
            log_fn(&format!("self-update {from} -> {to} installed; re-executing the new binary"));
            let exec_err = crate::self_update::reexec();
            log_fn(&format!(
                "re-exec after self-update failed: {exec_err}; continuing on the old in-memory binary"
            ));
        }
        Ok(crate::self_update::UpdateOutcome::UpToDate { .. }) => {
            // latest.json moved between detect_drift and self_update
            // (or detect raced a republish): nothing to do.
            log_fn("drift resolved before self-update ran (latest.json no longer newer)");
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
        assert_eq!(latest_release_from_json(&serde_json::json!({"releases": {}})), None);
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
            (Instant::now() - CACHE_TTL - Duration::from_secs(1), "0.2.0".to_string()),
        );
        assert_eq!(pypi_latest_at(&url, pkg).await.as_deref(), Some("0.2.0"));
        // Cleanup so no other test observes the seeded entry.
        PYPI_CACHE.lock().unwrap().remove(pkg);
    }
}
