//! Binary self-update from the release bucket.
//!
//! Closes the phase-4 gap called out in
//! [`crate::providers::local::version_check`]: Python agents remediated
//! version drift with `pip install --upgrade stado` + `os.execv`; the Rust
//! binary is not pip-installed, so drift detection compares
//! `CARGO_PKG_VERSION` against the pointer object
//! `gs://wisent-compute/releases/stado/latest.json`
//! (`{"version": "X.Y.Z", "channel": "stable"}`) — PyPI is deliberately
//! NOT consulted, because the `stado` PyPI package tracks the Python
//! implementation, whose version numbers no longer describe the running
//! Rust binary.
//!
//! Release layout (written by the release pipeline):
//!
//! ```text
//! gs://wisent-compute/releases/stado/latest.json
//! gs://wisent-compute/releases/stado/<version>/<platform>/stado
//! gs://wisent-compute/releases/stado/<version>/<platform>/wc
//! gs://wisent-compute/releases/stado/<version>/<platform>/stado-coverage
//! gs://wisent-compute/releases/stado/<version>/<platform>/stado-fix
//! gs://wisent-compute/releases/stado/<version>/<platform>/stado-watchdog
//! gs://wisent-compute/releases/stado/<version>/<platform>/stado-mcp
//! gs://wisent-compute/releases/stado/<version>/<platform>/SHA256SUMS
//! ```
//!
//! `platform` is [`platform_triple_short`] (`linux-amd64` or
//! `darwin-arm64`); SHA256SUMS is coreutils format (`<hash>  <name>`).
//!
//! Remediation ([`self_update`]) downloads the release for the installed
//! platform into a temp dir on the same filesystem as the install dir,
//! verifies EVERY target binary against SHA256SUMS, then atomically
//! replaces (`<name>.new` + chmod 755 + fsync + rename) the running binary
//! AND its same-dir siblings that exist among [`RELEASE_BINARIES`].
//! Failure stance: any error (bucket unreachable, malformed sums, hash
//! mismatch, unwritable install dir, unsupported platform) aborts BEFORE
//! the first rename — the old binaries stay in place and the caller keeps
//! running. Extra files in the install dir are never touched; sibling
//! binaries that are missing stay missing.
//!
//! After a successful update the agent calls [`reexec`], the
//! `os.execv` equivalent: the new binary replaces the process image with
//! the original argv and the inherited environment.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::providers::local::version_check::version_newer;
use crate::queue::{BlobBackend, GcsBackend};

/// Bucket the release pipeline publishes to. Deliberately NOT
/// `config::bucket()` — that is the QUEUE bucket (default "stado"); the
/// release tree lives in "wisent-compute" regardless of queue config.
pub const RELEASE_BUCKET: &str = "wisent-compute";

/// Prefix of the release tree inside [`RELEASE_BUCKET`].
pub const RELEASE_PREFIX: &str = "releases/stado";

/// Pointer object republished by the release pipeline on every release.
pub const LATEST_JSON_PATH: &str = "releases/stado/latest.json";

/// The checksum manifest inside each `<version>/<platform>/` directory.
pub const SHA256SUMS_NAME: &str = "SHA256SUMS";

/// Binaries published per release/platform. The self-update replaces the
/// running binary plus the same-dir siblings among these names that exist.
pub const RELEASE_BINARIES: [&str; 6] =
    ["stado", "wc", "stado-coverage", "stado-fix", "stado-watchdog", "stado-mcp"];

/// Self-update failure. Every variant aborts before any binary is
/// replaced; the caller logs and keeps running the old binary.
#[derive(Debug, thiserror::Error)]
pub enum SelfUpdateError {
    /// This OS/arch has no published release triple.
    #[error("no release triple for platform {os}-{arch} (supported: linux-amd64, darwin-arm64)")]
    UnsupportedPlatform { os: &'static str, arch: &'static str },
    /// The release bucket/object could not be read.
    #[error("release fetch failed: {0}")]
    Fetch(String),
    /// SHA256SUMS is not parseable coreutils format.
    #[error("malformed SHA256SUMS: {0}")]
    MalformedSums(String),
    /// SHA256SUMS carries no entry for a binary we need to replace.
    #[error("SHA256SUMS has no entry for {0}")]
    MissingSum(String),
    /// A downloaded binary does not match its published checksum.
    #[error("sha256 mismatch for {name}: expected {expected}, got {actual}")]
    HashMismatch { name: String, expected: String, actual: String },
    /// `current_exe` has no usable parent directory.
    #[error("cannot determine install dir of {path}", path = .0.display())]
    NoInstallDir(PathBuf),
    /// The running binary's file name is not a published release binary,
    /// so there is nothing safe to download over it.
    #[error("running binary {0:?} is not one of the published release binaries")]
    UnknownBinary(String),
    /// Probed by creating the staging temp dir inside the install dir
    /// before any download starts.
    #[error("install dir {path} is not writable: {1}", path = .0.display())]
    InstallDirNotWritable(PathBuf, String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The parsed `latest.json` pointer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct LatestRelease {
    pub version: String,
    #[serde(default = "default_channel")]
    pub channel: String,
}

fn default_channel() -> String {
    "stable".to_string()
}

/// What [`self_update`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// The release was downloaded, verified and installed; the caller
    /// should [`reexec`].
    Updated { from: String, to: String },
    /// latest.json is not strictly newer than `CARGO_PKG_VERSION`
    /// (detection/update race, or a same-version pointer).
    UpToDate { installed: String, latest: String },
}

/// Release triple for the platforms the release pipeline publishes.
/// Other OS/arch combinations are a hard [`SelfUpdateError::UnsupportedPlatform`].
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn platform_triple_short() -> Result<&'static str, SelfUpdateError> {
    Ok("linux-amd64")
}

/// Release triple for the platforms the release pipeline publishes.
/// Other OS/arch combinations are a hard [`SelfUpdateError::UnsupportedPlatform`].
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn platform_triple_short() -> Result<&'static str, SelfUpdateError> {
    Ok("darwin-arm64")
}

/// Release triple for the platforms the release pipeline publishes.
/// Other OS/arch combinations are a hard [`SelfUpdateError::UnsupportedPlatform`].
#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
pub fn platform_triple_short() -> Result<&'static str, SelfUpdateError> {
    Err(SelfUpdateError::UnsupportedPlatform {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    })
}

/// Download seam for the release tree, so tests can run fully offline.
/// `object_path` is bucket-relative (e.g. [`LATEST_JSON_PATH`]).
#[async_trait]
pub trait ReleaseFetcher: Send + Sync {
    /// Object bytes, or `None` when the object does not exist.
    async fn fetch(&self, object_path: &str) -> Result<Option<Vec<u8>>, SelfUpdateError>;
}

/// [`ReleaseFetcher`] over the crate's GCS JSON-API backend, bound to
/// [`RELEASE_BUCKET`] explicitly (NOT the queue bucket).
pub struct GcsReleaseFetcher {
    backend: GcsBackend,
}

impl GcsReleaseFetcher {
    /// Resolve GCP credentials (same gcp_auth pattern as the queue
    /// backend) and bind to [`RELEASE_BUCKET`].
    pub async fn new() -> Result<Self, SelfUpdateError> {
        let backend = GcsBackend::new(RELEASE_BUCKET)
            .await
            .map_err(|err| SelfUpdateError::Fetch(err.to_string()))?;
        Ok(Self { backend })
    }
}

#[async_trait]
impl ReleaseFetcher for GcsReleaseFetcher {
    async fn fetch(&self, object_path: &str) -> Result<Option<Vec<u8>>, SelfUpdateError> {
        self.backend
            .download_bytes(object_path)
            .await
            .map_err(|err| SelfUpdateError::Fetch(err.to_string()))
    }
}

/// Parse a `latest.json` body; `None` on malformed JSON or a
/// missing/non-string `version`.
pub fn parse_latest_json(body: &str) -> Option<LatestRelease> {
    let parsed: LatestRelease = serde_json::from_str(body).ok()?;
    if parsed.version.is_empty() {
        return None;
    }
    Some(parsed)
}

/// Fetch and parse `latest.json` from the release bucket. `None` on any
/// failure — drift detection must never crash the agent loop (same
/// stance as the old PyPI fetch).
pub async fn check_latest() -> Option<LatestRelease> {
    let fetcher = GcsReleaseFetcher::new().await.ok()?;
    check_latest_with(&fetcher).await
}

/// [`check_latest`] against an injected fetcher (tests).
pub async fn check_latest_with(fetcher: &impl ReleaseFetcher) -> Option<LatestRelease> {
    let bytes = fetcher.fetch(LATEST_JSON_PATH).await.ok()??;
    parse_latest_json(std::str::from_utf8(&bytes).ok()?)
}

/// True when `latest` is strictly newer than the compiled-in crate
/// version (`CARGO_PKG_VERSION`). Reuses the Python-parity tuple compare
/// from the version_check module.
pub fn newer_than_installed(latest: &str) -> bool {
    version_newer(env!("CARGO_PKG_VERSION"), latest)
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// Parse a coreutils-format SHA256SUMS body into name -> lowercase hex
/// hash. Accepts both text-mode (`hash  name`) and binary-mode
/// (`hash *name`) markers. Blank lines are skipped; a malformed line is a
/// hard error — a truncated manifest must not silently shrink the
/// verified set.
pub fn parse_sha256sums(body: &str) -> Result<BTreeMap<String, String>, SelfUpdateError> {
    let mut out = BTreeMap::new();
    for (lineno, raw) in body.lines().enumerate() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        let Some(split_at) = line.find(char::is_whitespace) else {
            return Err(SelfUpdateError::MalformedSums(format!("line {}: {line:?}", lineno + 1)));
        };
        let hash = &line[..split_at];
        let name = line[split_at..].trim_start();
        let name = name.strip_prefix('*').unwrap_or(name);
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) || name.is_empty() {
            return Err(SelfUpdateError::MalformedSums(format!("line {}: {line:?}", lineno + 1)));
        }
        out.insert(name.to_string(), hash.to_ascii_lowercase());
    }
    Ok(out)
}

/// Names among [`RELEASE_BINARIES`] to replace: the running binary plus
/// the same-dir siblings that exist. Missing siblings stay missing;
/// nothing outside the six published names is ever touched.
pub fn update_targets(install_dir: &Path, current_exe: &Path) -> Result<Vec<String>, SelfUpdateError> {
    let exe_name = current_exe
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| SelfUpdateError::NoInstallDir(current_exe.to_path_buf()))?;
    if !RELEASE_BINARIES.contains(&exe_name) {
        return Err(SelfUpdateError::UnknownBinary(exe_name.to_string()));
    }
    let mut targets = vec![exe_name.to_string()];
    for name in RELEASE_BINARIES {
        if name != exe_name && install_dir.join(name).is_file() {
            targets.push(name.to_string());
        }
    }
    Ok(targets)
}

/// `current_exe` tolerating the Linux `/proc/self/exe` readlink suffix:
/// once the running binary has been replaced via rename, the symlink
/// reads `<path> (deleted)`. Strip that suffix so [`reexec`] execs the
/// NEW binary at the original path, not a stale deleted-inode name.
fn current_exe_path() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    if exe.exists() {
        return Ok(exe);
    }
    if let Some(stripped) = exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        return Ok(PathBuf::from(stripped));
    }
    Ok(exe)
}

/// Full self-update against the real release bucket: fetch latest.json,
/// bail when not strictly newer, download + verify + atomically replace.
pub async fn self_update(log_fn: &mut dyn FnMut(&str)) -> Result<UpdateOutcome, SelfUpdateError> {
    let fetcher = GcsReleaseFetcher::new().await?;
    let current_exe = current_exe_path()?;
    let platform = platform_triple_short()?;
    self_update_with(&fetcher, platform, &current_exe, log_fn).await
}

/// [`self_update`] with the fetcher, platform and running-binary path
/// injected (offline tests).
pub async fn self_update_with(
    fetcher: &impl ReleaseFetcher,
    platform: &str,
    current_exe: &Path,
    log_fn: &mut dyn FnMut(&str),
) -> Result<UpdateOutcome, SelfUpdateError> {
    let installed = env!("CARGO_PKG_VERSION").to_string();
    let latest = check_latest_with(fetcher).await.ok_or_else(|| {
        SelfUpdateError::Fetch(format!("{LATEST_JSON_PATH} unreachable or invalid in gs://{RELEASE_BUCKET}"))
    })?;
    if !version_newer(&installed, &latest.version) {
        return Ok(UpdateOutcome::UpToDate { installed, latest: latest.version });
    }
    let to = latest.version;
    let install_dir = current_exe
        .parent()
        .ok_or_else(|| SelfUpdateError::NoInstallDir(current_exe.to_path_buf()))?;
    let targets = update_targets(install_dir, current_exe)?;

    // Staging temp dir on the SAME filesystem as the install dir (the
    // rename below is atomic only within one filesystem). Creating it is
    // also the writability probe: refuse BEFORE any download when the
    // install dir is read-only for us.
    let staging = tempfile::tempdir_in(install_dir)
        .map_err(|err| SelfUpdateError::InstallDirNotWritable(install_dir.to_path_buf(), err.to_string()))?;

    let prefix = format!("{RELEASE_PREFIX}/{to}/{platform}");
    let sums_bytes = fetcher
        .fetch(&format!("{prefix}/{SHA256SUMS_NAME}"))
        .await?
        .ok_or_else(|| SelfUpdateError::Fetch(format!("{prefix}/{SHA256SUMS_NAME} is missing")))?;
    let sums = parse_sha256sums(
        std::str::from_utf8(&sums_bytes)
            .map_err(|err| SelfUpdateError::MalformedSums(format!("not UTF-8: {err}")))?,
    )?;

    // Download + verify EVERY target before the first rename: a bad hash
    // or a missing object leaves the install dir completely untouched.
    let mut staged: Vec<(String, PathBuf)> = Vec::with_capacity(targets.len());
    for name in &targets {
        let expected = sums.get(name).ok_or_else(|| SelfUpdateError::MissingSum(name.clone()))?;
        let bytes = fetcher
            .fetch(&format!("{prefix}/{name}"))
            .await?
            .ok_or_else(|| SelfUpdateError::Fetch(format!("{prefix}/{name} is missing")))?;
        let actual = sha256_hex(&bytes);
        if actual != *expected {
            return Err(SelfUpdateError::HashMismatch {
                name: name.clone(),
                expected: expected.clone(),
                actual,
            });
        }
        let staged_path = staging.path().join(name);
        std::fs::write(&staged_path, &bytes)?;
        log_fn(&format!("self-update: verified {name} {to} ({} bytes)", bytes.len()));
        staged.push((name.clone(), staged_path));
    }

    // All verified: atomically replace one by one (`<name>.new` + chmod
    // 755 + fsync + rename over), then fsync the directory so the renames
    // themselves are durable.
    for (name, staged_path) in &staged {
        replace_verified(staged_path, &install_dir.join(name))?;
        log_fn(&format!("self-update: installed {name} {to}"));
    }
    std::fs::File::open(install_dir)?.sync_all()?;
    Ok(UpdateOutcome::Updated { from: installed, to })
}

/// Copy the verified staging file next to `dest` as `<name>.new`, chmod
/// 755, fsync, then rename over `dest` (atomic on the same filesystem).
/// A failed rename removes the `.new` scratch file; the old binary at
/// `dest` is untouched.
fn replace_verified(staged: &Path, dest: &Path) -> Result<(), SelfUpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let name = dest
        .file_name()
        .ok_or_else(|| SelfUpdateError::NoInstallDir(dest.to_path_buf()))?;
    let tmp = dest.with_file_name(format!("{}.new", name.to_string_lossy()));
    std::fs::copy(staged, &tmp)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::File::open(&tmp)?.sync_all()?;
    if let Err(err) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err.into());
    }
    Ok(())
}

/// Re-exec the (just-replaced) current binary with the original argv,
/// inheriting the current environment — Python's `os.execv(path, argv)`
/// semantics. NEVER returns on success: the process image is replaced.
/// The returned error means the exec failed and the caller should keep
/// running the old in-memory binary.
pub fn reexec() -> std::io::Error {
    use std::os::unix::process::CommandExt;
    match current_exe_path() {
        Ok(exe) => std::process::Command::new(exe).args(std::env::args_os().skip(1)).exec(),
        Err(err) => err,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Offline [`ReleaseFetcher`] backed by an in-memory object map.
    struct MockFetcher {
        objects: HashMap<String, Vec<u8>>,
    }

    #[async_trait]
    impl ReleaseFetcher for MockFetcher {
        async fn fetch(&self, object_path: &str) -> Result<Option<Vec<u8>>, SelfUpdateError> {
            Ok(self.objects.get(object_path).cloned())
        }
    }

    /// Build a mock release tree: latest.json at `version`, a SHA256SUMS
    /// over `binaries`, and every binary object under
    /// `<version>/<platform>/`.
    fn mock_release(version: &str, platform: &str, binaries: &[(&str, &[u8])]) -> MockFetcher {
        let mut objects = HashMap::new();
        objects.insert(
            LATEST_JSON_PATH.to_string(),
            format!(r#"{{"version": "{version}", "channel": "stable"}}"#).into_bytes(),
        );
        let prefix = format!("{RELEASE_PREFIX}/{version}/{platform}");
        let sums = binaries
            .iter()
            .map(|(name, bytes)| format!("{}  {}", sha256_hex(bytes), name))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        objects.insert(format!("{prefix}/{SHA256SUMS_NAME}"), sums.into_bytes());
        for (name, bytes) in binaries {
            objects.insert(format!("{prefix}/{name}"), bytes.to_vec());
        }
        MockFetcher { objects }
    }

    fn no_log(_: &str) {}

    #[cfg(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    ))]
    #[test]
    fn platform_triple_short_maps_supported_host() {
        let triple = platform_triple_short().expect("supported host");
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        assert_eq!(triple, "linux-amd64");
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(triple, "darwin-arm64");
    }

    #[test]
    fn latest_json_parses_version_and_defaults_channel() {
        let parsed = parse_latest_json(r#"{"version": "0.4.393", "channel": "stable"}"#)
            .expect("valid latest.json");
        assert_eq!(parsed.version, "0.4.393");
        assert_eq!(parsed.channel, "stable");
        // channel optional (defaults to stable); version required.
        assert_eq!(parse_latest_json(r#"{"version": "1.0.0"}"#).unwrap().channel, "stable");
        assert!(parse_latest_json(r#"{"channel": "stable"}"#).is_none());
        assert!(parse_latest_json(r#"{"version": ""}"#).is_none());
        assert!(parse_latest_json("not json").is_none());
    }

    #[test]
    fn version_compare_uses_crate_version() {
        let installed = env!("CARGO_PKG_VERSION");
        assert!(!newer_than_installed(installed)); // same version: no update
        assert!(!newer_than_installed("0.0.1")); // older pointer: no update
        assert!(newer_than_installed(&format!("{installed}.1"))); // strictly newer
    }

    #[test]
    fn sha256sums_parses_coreutils_format() {
        let body = format!(
            "{}  stado\n{} *wc\n\n",
            "a".repeat(64),
            "B".repeat(64)
        );
        let sums = parse_sha256sums(&body).expect("valid sums");
        assert_eq!(sums["stado"], "a".repeat(64));
        assert_eq!(sums["wc"], "b".repeat(64)); // hex normalized to lowercase
        // Malformed lines are a hard error.
        assert!(parse_sha256sums("abc  stado\n").is_err()); // short hash
        assert!(parse_sha256sums(&format!("{}\n", "a".repeat(64))).is_err()); // no name
        assert!(parse_sha256sums("").unwrap().is_empty());
    }

    #[tokio::test]
    async fn self_update_replaces_siblings_and_keeps_extras() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path();
        let exe = install.join("stado");
        std::fs::write(&exe, b"old-stado").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(install.join("wc"), b"old-wc").unwrap();
        std::fs::write(install.join("notes.txt"), b"keep me").unwrap();

        let binaries: &[(&str, &[u8])] = &[
            ("stado", b"new-stado"),
            ("wc", b"new-wc"),
            ("stado-fix", b"new-fix"),
        ];
        let fetcher = mock_release("999.0.0", "test-platform", binaries);
        let mut logs: Vec<String> = Vec::new();
        let outcome = self_update_with(&fetcher, "test-platform", &exe, &mut |m: &str| {
            logs.push(m.to_string());
        })
        .await
        .expect("update succeeds");

        assert_eq!(
            outcome,
            UpdateOutcome::Updated { from: env!("CARGO_PKG_VERSION").to_string(), to: "999.0.0".into() }
        );
        // Running binary + existing sibling replaced...
        assert_eq!(std::fs::read(&exe).unwrap(), b"new-stado");
        assert_eq!(std::fs::read(install.join("wc")).unwrap(), b"new-wc");
        // ...chmod 755 applied...
        assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::metadata(install.join("wc")).unwrap().permissions().mode() & 0o777, 0o755);
        // ...extra files kept, missing siblings stay missing, no scratch
        // files (`.new` or staging dir) left behind.
        assert_eq!(std::fs::read(install.join("notes.txt")).unwrap(), b"keep me");
        assert!(!install.join("stado-fix").exists());
        let entries: Vec<String> = std::fs::read_dir(install)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries.len(), 3, "unexpected leftovers: {entries:?}");
        assert!(logs.iter().any(|m| m.contains("verified stado")));
    }

    #[tokio::test]
    async fn tampered_binary_replaces_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path();
        let exe = install.join("stado");
        std::fs::write(&exe, b"old-stado").unwrap();
        std::fs::write(install.join("wc"), b"old-wc").unwrap();

        let mut fetcher = mock_release(
            "999.0.0",
            "test-platform",
            &[("stado", b"new-stado"), ("wc", b"new-wc")],
        );
        // Corrupt the stado object AFTER the sums were computed.
        fetcher
            .objects
            .insert(format!("{RELEASE_PREFIX}/999.0.0/test-platform/stado"), b"tampered".to_vec());

        let err = self_update_with(&fetcher, "test-platform", &exe, &mut no_log)
            .await
            .expect_err("hash mismatch must abort");
        assert!(matches!(err, SelfUpdateError::HashMismatch { name, .. } if name == "stado"));
        // Rollback-free: every original byte is untouched.
        assert_eq!(std::fs::read(&exe).unwrap(), b"old-stado");
        assert_eq!(std::fs::read(install.join("wc")).unwrap(), b"old-wc");
        assert!(!install.join("stado.new").exists());
        assert!(!install.join("wc.new").exists());
    }

    #[tokio::test]
    async fn missing_sums_entry_replaces_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path();
        let exe = install.join("stado");
        std::fs::write(&exe, b"old-stado").unwrap();
        std::fs::write(install.join("wc"), b"old-wc").unwrap();
        // Manifest only covers stado; wc is a local sibling that needs replacing.
        let fetcher = mock_release("999.0.0", "test-platform", &[("stado", b"new-stado")]);
        let err = self_update_with(&fetcher, "test-platform", &exe, &mut no_log)
            .await
            .expect_err("missing sums entry must abort");
        assert!(matches!(err, SelfUpdateError::MissingSum(ref name) if name == "wc"));
        assert_eq!(std::fs::read(&exe).unwrap(), b"old-stado");
        assert_eq!(std::fs::read(install.join("wc")).unwrap(), b"old-wc");
    }

    #[tokio::test]
    async fn same_or_older_release_is_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("stado");
        std::fs::write(&exe, b"old-stado").unwrap();
        // Pointer at an older version than the compiled crate: no update.
        let fetcher = mock_release("0.0.1", "test-platform", &[("stado", b"new-stado")]);
        let outcome = self_update_with(&fetcher, "test-platform", &exe, &mut no_log)
            .await
            .expect("check succeeds");
        assert!(matches!(outcome, UpdateOutcome::UpToDate { .. }));
        assert_eq!(std::fs::read(&exe).unwrap(), b"old-stado");
    }

    #[tokio::test]
    async fn unknown_binary_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("stado-dev-build");
        std::fs::write(&exe, b"old").unwrap();
        let fetcher = mock_release("999.0.0", "test-platform", &[("stado", b"new")]);
        let err = self_update_with(&fetcher, "test-platform", &exe, &mut no_log)
            .await
            .expect_err("non-release binary name must refuse");
        assert!(matches!(err, SelfUpdateError::UnknownBinary(_)));
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    }

    #[tokio::test]
    async fn non_writable_install_dir_is_refused_before_any_replace() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path();
        let exe = install.join("stado");
        std::fs::write(&exe, b"old-stado").unwrap();
        let fetcher = mock_release("999.0.0", "test-platform", &[("stado", b"new-stado")]);
        std::fs::set_permissions(install, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root (and some ACL setups) ignore mode bits — probe the actual
        // capability instead of assuming the chmod makes the dir read-only.
        if tempfile::tempdir_in(install).is_ok() {
            std::fs::set_permissions(install, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let result = self_update_with(&fetcher, "test-platform", &exe, &mut no_log).await;
        // Restore writability so the TempDir can clean itself up.
        std::fs::set_permissions(install, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(result, Err(SelfUpdateError::InstallDirNotWritable(..))));
        assert_eq!(std::fs::read(&exe).unwrap(), b"old-stado");
    }

    #[tokio::test]
    async fn check_latest_with_reads_pointer_object() {
        let fetcher = mock_release("1.2.3", "test-platform", &[]);
        let latest = check_latest_with(&fetcher).await.expect("latest.json parses");
        assert_eq!(latest.version, "1.2.3");
        let empty = MockFetcher { objects: HashMap::new() };
        assert!(check_latest_with(&empty).await.is_none());
    }
}
