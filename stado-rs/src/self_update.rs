//! Binary self-update from an exact, immutable Stado release coordinate.
//!
//! The operator configures the public Stado release API together with one
//! exact version and platform. There is no mutable channel pointer, bucket
//! fallback, or provider credential path. Each requested object is addressed
//! as `stado://releases/stado/<version>/<platform>/<name>` through
//! `/api/release/object`.
//!
//! Remediation downloads the checksum manifest and every installed published
//! binary into a temporary directory on the install filesystem, verifies all
//! hashes, then atomically replaces the binaries. Any configuration, fetch,
//! checksum, or filesystem failure aborts before the first rename.
//!
//! After a successful update the agent calls [`reexec`], replacing the process
//! image with the new binary while preserving argv and the environment.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::providers::local::version_check::version_newer;

/// Legacy pointer name retained only by offline compatibility tests. Runtime
/// release resolution never fetches it.
#[cfg(test)]
pub const LATEST_JSON_NAME: &str = "latest.json";

/// The checksum manifest inside each `<version>/<platform>/` directory.
pub const SHA256SUMS_NAME: &str = "SHA256SUMS";

/// Binaries published per release/platform. The self-update replaces the
/// running binary plus the same-dir siblings among these names that exist.
pub const RELEASE_BINARIES: &[&str] = &[
    "stado",
    "wc",
    "stado-coverage",
    "stado-fix",
    "stado-watchdog",
    "stado-mcp",
];

/// Self-update failure. Every variant aborts before any binary is
/// replaced; the caller logs and keeps running the old binary.
#[derive(Debug, thiserror::Error)]
pub enum SelfUpdateError {
    /// This OS/arch has no published release triple.
    #[error("no release triple for platform {os}-{arch} (supported: linux-amd64, darwin-arm64)")]
    UnsupportedPlatform {
        os: &'static str,
        arch: &'static str,
    },
    /// The release channel/object could not be read.
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
    HashMismatch {
        name: String,
        expected: String,
        actual: String,
    },
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

/// Legacy pointer representation retained only for offline compatibility tests.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct LatestRelease {
    pub version: String,
    #[serde(default = "default_channel")]
    pub channel: String,
}

#[cfg(test)]
fn default_channel() -> String {
    "stable".to_string()
}

/// What [`self_update`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// The release was downloaded, verified and installed; the caller
    /// should [`reexec`].
    Updated { from: String, to: String },
    /// The configured exact version is not strictly newer than the installed
    /// binary.
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

/// Download seam for exact release objects. Runtime implementations accept
/// only `<version>/<platform>/<name>` under their configured coordinate.
#[async_trait]
pub trait ReleaseFetcher: Send + Sync {
    /// Object bytes, or `None` when the object does not exist.
    async fn fetch(&self, object_path: &str) -> Result<Option<Vec<u8>>, SelfUpdateError>;
}

fn canonical_coordinate(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn release_coordinates_error(api_url: &str, version: &str, platform: &str) -> Option<String> {
    if !canonical_coordinate(version) || !canonical_coordinate(platform) {
        return Some(
            "release.version and release.platform must be exact non-empty coordinates".to_string(),
        );
    }
    if api_url.contains('<') && api_url.contains('>') {
        return Some("release.api_url contains an unresolved placeholder".to_string());
    }
    let parsed = match url::Url::parse(api_url) {
        Ok(parsed) => parsed,
        Err(error) => return Some(format!("release.api_url is not an absolute URL: {error}")),
    };
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || (parsed.path() != "/" && !parsed.path().is_empty())
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Some(
            "release.api_url must be an HTTPS origin without credentials, query, or fragment"
                .to_string(),
        );
    }
    None
}

/// Public HTTPS Stado object-route fetcher bound to one exact configured
/// version and platform.
pub struct HttpReleaseFetcher {
    http: reqwest::Client,
    api_url: String,
    version: String,
    platform: String,
    configuration_error: Option<String>,
}

impl HttpReleaseFetcher {
    /// Bind every fetch to the configured immutable release coordinate.
    pub fn new() -> Self {
        let api_url = crate::config::stado_release_api_url();
        let version = crate::config::stado_release_version();
        let platform = crate::config::stado_release_platform();
        let configuration_error = release_coordinates_error(&api_url, &version, &platform);
        Self {
            http: reqwest::Client::new(),
            api_url,
            version,
            platform,
            configuration_error,
        }
    }
}

impl Default for HttpReleaseFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ReleaseFetcher for HttpReleaseFetcher {
    async fn fetch(&self, object_path: &str) -> Result<Option<Vec<u8>>, SelfUpdateError> {
        if let Some(error) = &self.configuration_error {
            return Err(SelfUpdateError::Fetch(error.clone()));
        }
        let expected_prefix = format!("{}/{}/", self.version, self.platform);
        let Some(name) = object_path.strip_prefix(&expected_prefix) else {
            return Err(SelfUpdateError::Fetch(
                "release object is outside the configured version/platform".to_string(),
            ));
        };
        if name.is_empty() || name.contains('/') {
            return Err(SelfUpdateError::Fetch(
                "release object name must be one exact path segment".to_string(),
            ));
        }
        let release_uri = format!("stado://releases/stado/{object_path}");
        let mut endpoint = url::Url::parse(&self.api_url)
            .and_then(|base| base.join("/api/release/object"))
            .map_err(|error| SelfUpdateError::Fetch(format!("invalid release API: {error}")))?;
        endpoint.query_pairs_mut().append_pair("uri", &release_uri);
        let response = self
            .http
            .get(endpoint.clone())
            .send()
            .await
            .map_err(|error| SelfUpdateError::Fetch(format!("{endpoint}: {error}")))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(SelfUpdateError::Fetch(format!(
                "{endpoint} -> HTTP {status}"
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| SelfUpdateError::Fetch(format!("{endpoint}: {error}")))?;
        Ok(Some(bytes.to_vec()))
    }
}

/// Parse a legacy pointer body for offline compatibility tests only.
#[cfg(test)]
pub fn parse_latest_json(body: &str) -> Option<LatestRelease> {
    let parsed: LatestRelease = serde_json::from_str(body).ok()?;
    if parsed.version.is_empty() {
        return None;
    }
    Some(parsed)
}

#[cfg(test)]
pub async fn check_latest_with(fetcher: &impl ReleaseFetcher) -> Option<LatestRelease> {
    let bytes = fetcher.fetch(LATEST_JSON_NAME).await.ok()??;
    parse_latest_json(std::str::from_utf8(&bytes).ok()?)
}

#[cfg(test)]
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
            return Err(SelfUpdateError::MalformedSums(format!(
                "line {}: {line:?}",
                lineno + 1
            )));
        };
        let hash = &line[..split_at];
        let name = line[split_at..].trim_start();
        let name = name.strip_prefix('*').unwrap_or(name);
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) || name.is_empty() {
            return Err(SelfUpdateError::MalformedSums(format!(
                "line {}: {line:?}",
                lineno + 1
            )));
        }
        out.insert(name.to_string(), hash.to_ascii_lowercase());
    }
    Ok(out)
}

/// Names among [`RELEASE_BINARIES`] to replace: the running binary plus
/// the same-dir siblings that exist. Missing siblings stay missing;
/// nothing outside the published names is ever touched.
pub fn update_targets(
    install_dir: &Path,
    current_exe: &Path,
) -> Result<Vec<String>, SelfUpdateError> {
    let exe_name = current_exe
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| SelfUpdateError::NoInstallDir(current_exe.to_path_buf()))?;
    if !RELEASE_BINARIES.contains(&exe_name) {
        return Err(SelfUpdateError::UnknownBinary(exe_name.to_string()));
    }
    let mut targets = vec![exe_name.to_string()];
    for &name in RELEASE_BINARIES {
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

/// Install the configured exact release when it is newer than this binary.
pub async fn self_update(log_fn: &mut dyn FnMut(&str)) -> Result<UpdateOutcome, SelfUpdateError> {
    let fetcher = HttpReleaseFetcher::new();
    if let Some(error) = &fetcher.configuration_error {
        return Err(SelfUpdateError::Fetch(error.clone()));
    }
    let current_exe = current_exe_path()?;
    let host_platform = platform_triple_short()?;
    if fetcher.platform != host_platform {
        return Err(SelfUpdateError::Fetch(format!(
            "configured release platform {:?} does not match this host {:?}",
            fetcher.platform, host_platform
        )));
    }
    let installed = env!("CARGO_PKG_VERSION").to_string();
    let to = fetcher.version.clone();
    if !version_newer(&installed, &to) {
        return Ok(UpdateOutcome::UpToDate {
            installed,
            latest: to,
        });
    }
    install_release_with(&fetcher, installed, to, host_platform, &current_exe, log_fn).await
}

/// Legacy injected pointer seam retained only for offline compatibility tests.
#[cfg(test)]
pub async fn self_update_with(
    fetcher: &impl ReleaseFetcher,
    platform: &str,
    current_exe: &Path,
    log_fn: &mut dyn FnMut(&str),
) -> Result<UpdateOutcome, SelfUpdateError> {
    let installed = env!("CARGO_PKG_VERSION").to_string();
    let latest = check_latest_with(fetcher).await.ok_or_else(|| {
        SelfUpdateError::Fetch("legacy test release pointer is missing".to_string())
    })?;
    if !version_newer(&installed, &latest.version) {
        return Ok(UpdateOutcome::UpToDate {
            installed,
            latest: latest.version,
        });
    }
    install_release_with(
        fetcher,
        installed,
        latest.version,
        platform,
        current_exe,
        log_fn,
    )
    .await
}

async fn install_release_with(
    fetcher: &impl ReleaseFetcher,
    installed: String,
    to: String,
    platform: &str,
    current_exe: &Path,
    log_fn: &mut dyn FnMut(&str),
) -> Result<UpdateOutcome, SelfUpdateError> {
    let install_dir = current_exe
        .parent()
        .ok_or_else(|| SelfUpdateError::NoInstallDir(current_exe.to_path_buf()))?;
    let targets = update_targets(install_dir, current_exe)?;
    let staging = tempfile::tempdir_in(install_dir).map_err(|error| {
        SelfUpdateError::InstallDirNotWritable(install_dir.to_path_buf(), error.to_string())
    })?;
    let prefix = format!("{to}/{platform}");
    let sums_bytes = fetcher
        .fetch(&format!("{prefix}/{SHA256SUMS_NAME}"))
        .await?
        .ok_or_else(|| SelfUpdateError::Fetch(format!("{prefix}/{SHA256SUMS_NAME} is missing")))?;
    let sums = parse_sha256sums(
        std::str::from_utf8(&sums_bytes)
            .map_err(|error| SelfUpdateError::MalformedSums(format!("not UTF-8: {error}")))?,
    )?;
    let mut staged: Vec<(String, PathBuf)> = Vec::with_capacity(targets.len());
    for name in &targets {
        let expected = sums
            .get(name)
            .ok_or_else(|| SelfUpdateError::MissingSum(name.clone()))?;
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
        log_fn(&format!(
            "self-update: verified {name} {to} ({} bytes)",
            bytes.len()
        ));
        staged.push((name.clone(), staged_path));
    }
    for (name, staged_path) in &staged {
        replace_verified(staged_path, &install_dir.join(name))?;
        log_fn(&format!("self-update: installed {name} {to}"));
    }
    std::fs::File::open(install_dir)?.sync_all()?;
    Ok(UpdateOutcome::Updated {
        from: installed,
        to,
    })
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
        Ok(exe) => std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .exec(),
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
            LATEST_JSON_NAME.to_string(),
            format!(r#"{{"version": "{version}", "channel": "stable"}}"#).into_bytes(),
        );
        let prefix = format!("{version}/{platform}");
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
        assert_eq!(
            parse_latest_json(r#"{"version": "1.0.0"}"#)
                .unwrap()
                .channel,
            "stable"
        );
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
        let body = format!("{}  stado\n{} *wc\n\n", "a".repeat(64), "B".repeat(64));
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
            UpdateOutcome::Updated {
                from: env!("CARGO_PKG_VERSION").to_string(),
                to: "999.0.0".into()
            }
        );
        // Running binary + existing sibling replaced...
        assert_eq!(std::fs::read(&exe).unwrap(), b"new-stado");
        assert_eq!(std::fs::read(install.join("wc")).unwrap(), b"new-wc");
        // ...chmod 755 applied...
        assert_eq!(
            std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(install.join("wc"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        // ...extra files kept, missing siblings stay missing, no scratch
        // files (`.new` or staging dir) left behind.
        assert_eq!(
            std::fs::read(install.join("notes.txt")).unwrap(),
            b"keep me"
        );
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
        fetcher.objects.insert(
            "999.0.0/test-platform/stado".to_string(),
            b"tampered".to_vec(),
        );

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
        assert!(matches!(
            result,
            Err(SelfUpdateError::InstallDirNotWritable(..))
        ));
        assert_eq!(std::fs::read(&exe).unwrap(), b"old-stado");
    }

    #[tokio::test]
    async fn check_latest_with_reads_pointer_object() {
        let fetcher = mock_release("1.2.3", "test-platform", &[]);
        let latest = check_latest_with(&fetcher)
            .await
            .expect("latest.json parses");
        assert_eq!(latest.version, "1.2.3");
        let empty = MockFetcher {
            objects: HashMap::new(),
        };
        assert!(check_latest_with(&empty).await.is_none());
    }
}
