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

use crate::release::{canonical_coordinate, version_newer};

/// Legacy pointer name retained only by offline compatibility tests. Runtime
/// release resolution never fetches it.

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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseArchiveManifest {
    product: String,
    version: String,
    platform: String,
    source_commit: String,
    sha256: String,
}

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

fn release_coordinates_error(api_url: &str, version: &str, platform: &str) -> Option<String> {
    if !canonical_coordinate(version) || !canonical_coordinate(platform) {
        return Some(
            "release.version and release.platform must be exact non-empty coordinates".to_string(),
        );
    }
    if api_url.contains('<') && api_url.contains('>') {
        return Some("api.url contains an unresolved placeholder".to_string());
    }
    let parsed = match url::Url::parse(api_url) {
        Ok(parsed) => parsed,
        Err(error) => return Some(format!("api.url is not an absolute URL: {error}")),
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
            "api.url must be an HTTPS origin without credentials, query, or fragment".to_string(),
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
        let api_url = crate::config::stado_api_url();
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
    let manifest_name = format!("release-manifest-{platform}.json");
    let manifest_bytes = fetcher
        .fetch(&format!("{prefix}/{manifest_name}"))
        .await?
        .ok_or_else(|| SelfUpdateError::Fetch(format!("{prefix}/{manifest_name} is missing")))?;
    let manifest: ReleaseArchiveManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| SelfUpdateError::Fetch(format!("invalid release manifest: {error}")))?;
    if manifest.product != "stado"
        || manifest.version != to
        || manifest.platform != platform
        || !matches!(manifest.source_commit.len(), 40 | 64)
        || !manifest
            .source_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || manifest.sha256.len() != 64
        || !manifest
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SelfUpdateError::Fetch(
            "release manifest identity or digest is invalid".to_string(),
        ));
    }
    let archive_name = format!("stado-v{to}-{platform}.tar.gz");
    let archive_bytes = fetcher
        .fetch(&format!("{prefix}/{archive_name}"))
        .await?
        .ok_or_else(|| SelfUpdateError::Fetch(format!("{prefix}/{archive_name} is missing")))?;
    let actual = sha256_hex(&archive_bytes);
    if actual != manifest.sha256 {
        return Err(SelfUpdateError::HashMismatch {
            name: archive_name,
            expected: manifest.sha256,
            actual,
        });
    }
    let extracted = staging.path().join("archive");
    crate::release_control::safe_extract_archive(&archive_bytes, &extracted)
        .map_err(SelfUpdateError::Fetch)?;
    let mut staged: Vec<(String, PathBuf)> = Vec::with_capacity(targets.len());
    for name in &targets {
        let staged_path = extracted.join(name);
        let metadata = std::fs::symlink_metadata(&staged_path)?;
        if !metadata.file_type().is_file() || metadata.len() == 0 {
            return Err(SelfUpdateError::Fetch(format!(
                "release archive member {name} is not a non-empty regular file"
            )));
        }
        log_fn(&format!("self-update: verified {name} {to}"));
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

