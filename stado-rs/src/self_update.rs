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
//!
//! `reexec` covers the process that ran the update and nothing else. Every
//! OTHER long-running unit installed from the same directory keeps executing
//! the inode it started with, so [`recycle_replaced_units`] restarts those in
//! place once the new binaries are on disk.

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
    // Leave the receipt the fleet's provenance check reads.
    //
    // `stado host release` stages every binary it delivers at
    // `$HOME/.stado/releases/<binary>/<version>/<platform>/<binary>`, and
    // `cli::service_converge::attest_installed` decides provenance by
    // comparing the installed file against exactly that path. Self-update is
    // the other delivery path and it staged nothing: it verified these bytes
    // against the published SHA-256 manifest a few lines above, installed
    // them, and threw the evidence away. So every binary self-update ever
    // delivered read `unattested` afterwards — the fleet had the provenance
    // and discarded it, then reported the result as if the bytes were
    // untrustworthy.
    //
    // On 2026-09-01 `lukasz-macbook` reported exactly that for `stado`: nine
    // versions staged by `host release`, the newest 0.13.24, and an installed
    // binary with no staged copy at all.
    //
    // Never fatal. The bytes are verified and the install is the point; a
    // receipt that cannot be written is logged and the update continues.
    for (name, staged_path) in &staged {
        if let Err(error) = stage_for_attestation(name, &to, platform, staged_path) {
            log_fn(&format!(
                "self-update: {name} {to} installed but its attestation copy could not be \
                 staged, so `stado service converge` will read it as unattested: {error}"
            ));
        }
    }
    for (name, staged_path) in &staged {
        replace_verified(staged_path, &install_dir.join(name))?;
        log_fn(&format!("self-update: installed {name} {to}"));
    }
    std::fs::File::open(install_dir)?.sync_all()?;
    if let Err(error) = recycle_replaced_units("self-update", install_dir, &targets, log_fn).await {
        log_fn(&format!("self-update: {error}"));
    }
    Ok(UpdateOutcome::Updated {
        from: installed,
        to,
    })
}

/// Copy one verified release member to the coordinate the fleet's provenance
/// check reads, creating `<binary>/<version>/<platform>/` beneath
/// `$HOME/.stado/releases`.
///
/// The bytes are the extracted archive member — the same file
/// [`replace_verified`] installs — so the staged copy is byte-identical to
/// the installed one and `cmp -s` in `attest_installed` matches. Written to a
/// dot-prefixed name and renamed, so a reader never sees a partial copy at
/// the coordinate it attests against.
pub(crate) fn stage_for_attestation(
    name: &str,
    version: &str,
    platform: &str,
    verified: &Path,
) -> Result<(), SelfUpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::var_os("HOME").ok_or_else(|| {
        SelfUpdateError::Fetch("HOME is unset, so the attestation copy has nowhere to go".into())
    })?;
    let coordinate = PathBuf::from(home)
        .join(".stado")
        .join("releases")
        .join(name)
        .join(version)
        .join(platform);
    std::fs::create_dir_all(&coordinate)?;
    let destination = coordinate.join(name);
    let temporary = coordinate.join(format!(".{name}.staging"));
    std::fs::copy(verified, &temporary)?;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))?;
    std::fs::File::open(&temporary)?.sync_all()?;
    if let Err(error) = std::fs::rename(&temporary, &destination) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

/// Reconcile OTHER managed units configured to run, or still executing,
/// the binaries this update just replaced.
///
/// Why this exists: [`replace_verified`] renames a new binary over the old
/// one, and only the process that ran the update re-execs itself. A unit that
/// is not that process keeps executing the inode it started with, for as long
/// as it lives, because neither launchd nor systemd has any reason to notice
/// that the file underneath it changed.
///
/// That is not theoretical. On 2026-09-01 the disk-cleanup janitor on
/// `lukasz-macbook` was executing a 68,977,488-byte image of
/// `~/.stado/bin/stado` while the file at that exact path was 70,892,848
/// bytes: the process had been up since 2026-08-27 and the binary was
/// replaced under it. Its reports named four cleaners and carried no
/// `writer_version`, while the installed build declares six and sets that
/// field, so the registry policy it was handed no longer validated. It
/// answered `invalid_or_unavailable_policy` 8,460 times out of 12,009 passes,
/// freed zero bytes across all of them, and the volume reached 100% with a
/// janitor running every minute the whole way down.
///
/// Prefer an in-place restart. A launchd definition whose program changed must
/// be reloaded in its observed owner domain; a kick would reuse the stale argv.
/// Both paths verify the resulting kernel image before delivery can succeed.
///
/// A failed reader refresh fails delivery even though the binary has already
/// been installed. Retrying delivery must finish the runtime half rather than
/// reporting success merely because the installed pathname is current.
///
/// The queue agent is deliberately left alone. `cli::release_cmd::install_local`
/// writes `~/.stado/bin/stado.release-version`, and
/// `providers::local::agent` compares that file with the version it was
/// compiled from, finishes the slot it is holding, and lets its supervisor
/// recreate it. Kicking it here would abort a running job to save it a few
/// minutes, so a unit whose argv carries the `agent` subcommand is reported
/// and skipped. That handshake is the only one any unit implements, which is
/// why every other unit needs this function at all.
///
/// `context` prefixes every line, because both delivery paths call this and a
/// log that says `self-update` about a `release install-local` is a lie.
pub(crate) async fn recycle_replaced_units(
    context: &str,
    install_dir: &Path,
    replaced: &[String],
    log_fn: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let paths: Vec<String> = replaced
        .iter()
        .map(|name| install_dir.join(name).to_string_lossy().into_owned())
        .collect();
    let ours = std::process::id();
    let restarted = if cfg!(target_os = "macos") {
        recycle_launchd(context, &paths, ours, log_fn).await?
    } else {
        recycle_systemd(context, &paths, ours, log_fn).await?
    };
    if restarted == 0 {
        log_fn(&format!(
            "{context}: no other managed unit was running a replaced binary"
        ));
    }
    Ok(())
}

/// Whether an argv belongs to the queue agent, which recycles itself through
/// the installed-release handshake and must not be kicked mid-slot.
///
/// Crate-visible because it is the one place this exclusion is written down.
/// `release_unit_image::revisit_plan` applies the same rule on a
/// schedule, and the fleet agent is one of the units that goes stale, so a
/// second spelling of "which units recycle themselves" is a second answer
/// waiting to disagree with this one.
pub(crate) fn defers_to_release_handshake<S: AsRef<str>>(argv: &[S]) -> bool {
    let mut arguments = argv.iter().skip(1).map(AsRef::as_ref);
    let first = arguments.next();
    let subcommand = if first == Some("--") {
        arguments.next()
    } else {
        first
    };
    subcommand == Some("agent")
}

/// `launchctl list` prints `PID\tStatus\tLabel` after one header row. A job
/// that is loaded but not running prints `-` for the PID and holds no image,
/// so it is skipped: it will pick up the new binary the next time launchd
/// starts it.
async fn recycle_launchd(
    context: &str,
    paths: &[String],
    ours: u32,
    log_fn: &mut dyn FnMut(&str),
) -> Result<usize, String> {
    let registry = crate::cli::registry::read_registry()
        .await
        .map_err(|error| format!("{context}: cannot read registry unit ownership: {error}"))?;
    let hostname = crate::providers::vast::system_hostname();
    let target = registry
        .lookup_self(&hostname)
        .map_err(|error| format!("{context}: cannot identify this host: {error}"))?
        .ok_or_else(|| format!("{context}: no registry target names this machine ({hostname})"))?;
    let runner = crate::deploy::production_runner();
    let units = crate::deploy::service::loaded_units(target, &runner)
        .await
        .map_err(|error| {
            format!("{context}: cannot enumerate domain-bound launchd units: {error}")
        })?;
    let pids: Vec<u32> = units
        .iter()
        .filter_map(|unit| unit.pid.parse().ok())
        .filter(|pid| *pid != ours)
        .collect();
    let running_images = crate::deploy::service::running_images(&pids)
        .map_err(|error| format!("{context}: cannot read running image identities: {error}"))?;
    let installed_images: Vec<(String, crate::deploy::service::ImageIdentity)> = paths
        .iter()
        .map(|path| {
            crate::deploy::service::installed_image(Path::new(path))
                .map(|(image, _)| (path.clone(), image))
                .map_err(|error| format!("{context}: cannot identify installed {path}: {error}"))
        })
        .collect::<Result<_, _>>()?;
    let mut restarted = 0usize;
    for unit in units {
        let Ok(pid) = unit.pid.parse::<u32>() else {
            continue;
        };
        if pid == ours {
            continue;
        }
        let running = running_images.get(&pid);
        let declared_program = unit.program.split_whitespace().next();
        let directly_declared = paths
            .iter()
            .any(|path| declared_program == Some(path.as_str()));
        if directly_declared && running.is_none() {
            return Err(format!(
                "{context}: the kernel image for {} pid {pid} is unreadable",
                unit.label
            ));
        }
        let selected = running.and_then(|running| {
            installed_images.iter().find(|(path, installed)| {
                (declared_program == Some(path.as_str())
                    || running.path.trim_end_matches(" (deleted)") == path)
                    && !running.is_same_file(installed)
            })
        });
        let Some((program, installed)) = selected else {
            continue;
        };
        let argv: Vec<&str> = unit.running_program.split_whitespace().collect();
        if defers_to_release_handshake(&argv) {
            log_fn(&format!(
                "{context}: {} is running the replaced {program} and recycles itself through \
                 the installed-release handshake, so it was left to finish its slot",
                unit.label
            ));
            continue;
        }
        if unit.loaded_domains.len() != 1 {
            return Err(format!(
                "{context}: {} pid {pid} executes replaced {program}, but launchd reports {} \
                 loaded domains; refusing to guess which job owns the pid",
                unit.label,
                unit.loaded_domains.len()
            ));
        }
        let service = crate::deploy::service::restart_local_unit(
            target,
            &unit.label,
            &unit.path,
            Some(&unit.loaded_domains[0]),
        )
        .await
        .map_err(|error| {
            format!(
                "{context}: {} was executing the replaced {program} and could not be restarted \
                 through its observed owner {}/{} and declared unit {}: {error}",
                unit.label, unit.loaded_domains[0], unit.label, unit.path
            )
        })?;
        let after = crate::deploy::service::loaded_units(target, &runner)
            .await
            .map_err(|error| format!("{context}: cannot re-read {service}: {error}"))?;
        let current_pid = after
            .iter()
            .find(|current| {
                current.label == unit.label && current.loaded_domains == unit.loaded_domains
            })
            .and_then(|current| current.pid.parse::<u32>().ok())
            .ok_or_else(|| format!("{context}: {service} restarted without a readable pid"))?;
        let images = crate::deploy::service::running_images(&[current_pid])
            .map_err(|error| format!("{context}: cannot verify {service}'s new image: {error}"))?;
        if !images
            .get(&current_pid)
            .is_some_and(|running| running.is_same_file(installed))
        {
            return Err(format!(
                "{context}: {service} restarted but pid {current_pid} does not execute the installed inode at {program}"
            ));
        }
        log_fn(&format!(
            "{context}: reconciled {service}; pid {pid} was running a different image, \
             and pid {current_pid} now executes the installed inode at {program}"
        ));
        restarted += 1;
    }
    Ok(restarted)
}

async fn systemctl_stdout(user: bool, args: &[&str]) -> Result<String, String> {
    let mut command = tokio::process::Command::new("systemctl");
    if user {
        // A system-scoped queue worker does not inherit a login's user-bus
        // environment. Address the same owner and runtime as service verbs.
        // SAFETY: geteuid has no arguments or memory preconditions.
        let uid = unsafe { nix::libc::geteuid() };
        let runtime = format!("/run/user/{uid}");
        command.arg("--user").env("XDG_RUNTIME_DIR", &runtime).env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={runtime}/bus"),
        );
    }
    let output = command
        .args(args)
        .output()
        .await
        .map_err(|error| format!("cannot execute systemctl: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "systemctl {} exited {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("systemctl returned invalid UTF-8: {error}"))
}

async fn recycle_systemd(
    context: &str,
    paths: &[String],
    ours: u32,
    log_fn: &mut dyn FnMut(&str),
) -> Result<usize, String> {
    let mut restarted = 0usize;
    for user in [false, true] {
        let listing = systemctl_stdout(
            user,
            &[
                "list-units",
                "--type=service",
                "--state=running",
                "--no-legend",
                "--plain",
            ],
        )
        .await
        .map_err(|error| {
            format!(
                "{context}: {} systemd manager did not enumerate running services: {error}",
                if user { "user" } else { "system" }
            )
        })?;
        for line in listing.lines() {
            let Some(unit) = line.split_whitespace().next() else {
                continue;
            };
            let show_args = ["show", "-p", "MainPID", "--value", unit];
            let main_pid = systemctl_stdout(user, &show_args)
                .await
                .map_err(|error| format!("{context}: {unit} did not report MainPID: {error}"))?
                .trim()
                .parse::<u32>()
                .map_err(|error| format!("{context}: {unit} reported invalid MainPID: {error}"))?;
            if main_pid == 0 {
                return Err(format!(
                    "{context}: {unit} was listed running but reported MainPID=0"
                ));
            }
            if main_pid == ours {
                continue;
            }
            let images = crate::deploy::service::running_images(&[main_pid]).map_err(|error| {
                format!("{context}: cannot read {unit} pid {main_pid}: {error}")
            })?;
            let running = images
                .get(&main_pid)
                .ok_or_else(|| format!("{context}: no kernel image for {unit} pid {main_pid}"))?;
            let Some(path) = paths
                .iter()
                .find(|path| running.path.trim_end_matches(" (deleted)") == path.as_str())
            else {
                continue;
            };
            let (installed, _) = crate::deploy::service::installed_image(Path::new(path))
                .map_err(|error| format!("{context}: cannot identify installed {path}: {error}"))?;
            if running.is_same_file(&installed) {
                continue;
            }
            let argv = crate::deploy::service::process_table()
                .ok()
                .and_then(|rows| {
                    rows.into_iter()
                        .find(|(pid, _, _)| *pid == main_pid)
                        .map(|(_, _, argv)| argv)
                })
                .ok_or_else(|| format!("{context}: cannot read argv for {unit} pid {main_pid}"));
            let argv = argv?;
            let tokens: Vec<&str> = argv.split_whitespace().collect();
            if defers_to_release_handshake(&tokens) {
                log_fn(&format!(
                    "{context}: {unit} is the queue agent and defers to its installed-release handshake"
                ));
                continue;
            }
            systemctl_stdout(user, &["try-restart", unit])
                .await
                .map_err(|error| {
                    format!(
                        "{context}: {unit} pid {main_pid} could not be restarted onto the installed inode: {error}"
                    )
                })?;
            let mut replacement = None;
            for _ in 0..60 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let Some(new_pid) = systemctl_stdout(user, &show_args)
                    .await
                    .ok()
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .filter(|pid| *pid != 0)
                else {
                    continue;
                };
                let verified = crate::deploy::service::running_images(&[new_pid])
                    .ok()
                    .is_some_and(|images| {
                        images
                            .get(&new_pid)
                            .is_some_and(|image| image.is_same_file(&installed))
                    });
                if verified {
                    replacement = Some(new_pid);
                    break;
                }
            }
            let Some(new_pid) = replacement else {
                return Err(format!(
                    "{context}: {unit} restarted but no replacement pid mapped the installed inode within 30s"
                ));
            };
            log_fn(&format!(
                "{context}: restarted {unit}; pid {main_pid} held the replaced inode and pid {new_pid} maps the installed inode"
            ));
            restarted += 1;
        }
    }
    Ok(restarted)
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
