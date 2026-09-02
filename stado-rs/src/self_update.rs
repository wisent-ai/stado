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
    recycle_replaced_units("self-update", install_dir, &targets, log_fn).await;
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
fn stage_for_attestation(
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

/// Restart the OTHER managed units that were executing the binaries this
/// update just replaced.
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
/// In place only. `launchctl kickstart -k` and `systemctl --user try-restart`
/// replace the process without unloading the unit, so there is no window in
/// which the job does not exist. The unload-and-bootstrap sequence took the
/// always-on host down once already and is deliberately not reached from here.
///
/// Best effort by construction. The binaries are installed and fsynced before
/// this runs, so a unit that cannot be restarted must not fail the update that
/// already succeeded; it is named in the log and left holding the old image.
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
) {
    let paths: Vec<String> = replaced
        .iter()
        .map(|name| install_dir.join(name).to_string_lossy().into_owned())
        .collect();
    let ours = std::process::id();
    let restarted = if cfg!(target_os = "macos") {
        recycle_launchd(context, &paths, ours, log_fn).await
    } else {
        recycle_systemd(context, &paths, ours, log_fn).await
    };
    if restarted == 0 {
        log_fn(&format!(
            "{context}: no other managed unit was running a replaced binary"
        ));
    }
}

/// Whether an argv belongs to the queue agent, which recycles itself through
/// the installed-release handshake and must not be kicked mid-slot.
fn defers_to_release_handshake<S: AsRef<str>>(argv: &[S]) -> bool {
    argv.iter().skip(1).any(|token| token.as_ref() == "agent")
}

/// Stdout of a successful command, or `None` for a missing binary, a spawn
/// failure, a nonzero exit, or non-UTF-8 output. The caller treats all four
/// the same way: it did not happen, and the log says so.
async fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// This account's uid, read from the ownership of its own home directory, so
/// that naming a launchd domain costs neither a new crate feature nor a
/// subprocess.
fn unix_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    let home = std::env::var_os("HOME")?;
    Some(std::fs::metadata(home).ok()?.uid())
}

/// The argv of one launchd unit: `ProgramArguments` when it has one, else the
/// single `Program`. Read through `plutil`, which every macOS carries, so no
/// plist parser joins the dependency set for the sake of two fields.
///
/// The whole argv, not just the program, because the subcommand decides
/// whether a unit recycles itself — see [`defers_to_release_handshake`].
async fn launchd_argv(plist: &Path) -> Option<Vec<String>> {
    let path = plist.to_string_lossy().into_owned();
    let rendered =
        command_stdout("/usr/bin/plutil", &["-convert", "json", "-o", "-", &path]).await?;
    let value: serde_json::Value = serde_json::from_str(&rendered).ok()?;
    if let Some(arguments) = value
        .get("ProgramArguments")
        .and_then(serde_json::Value::as_array)
    {
        let argv: Vec<String> = arguments
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect();
        if !argv.is_empty() {
            return Some(argv);
        }
    }
    value
        .get("Program")
        .and_then(serde_json::Value::as_str)
        .map(|program| vec![program.to_string()])
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
) -> usize {
    let Some(uid) = unix_uid() else {
        log_fn(&format!(
            "{context}: this account's uid is unreadable, so no launchd unit was restarted"
        ));
        return 0;
    };
    let Some(listing) = command_stdout("/bin/launchctl", &["list"]).await else {
        log_fn(&format!(
            "{context}: launchctl list failed, so no launchd unit was restarted"
        ));
        return 0;
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let domains = [
        (
            PathBuf::from(&home).join("Library").join("LaunchAgents"),
            format!("gui/{uid}"),
        ),
        (
            PathBuf::from("/Library/LaunchDaemons"),
            "system".to_string(),
        ),
    ];
    let mut restarted = 0usize;
    for line in listing.lines().skip(1) {
        let mut columns = line.split('\t');
        let (Some(pid), Some(_status), Some(label)) =
            (columns.next(), columns.next(), columns.next())
        else {
            continue;
        };
        let Ok(pid) = pid.trim().parse::<u32>() else {
            continue;
        };
        if pid == ours {
            continue;
        }
        for (directory, domain) in &domains {
            let plist = directory.join(format!("{label}.plist"));
            if !plist.is_file() {
                continue;
            }
            let Some(argv) = launchd_argv(&plist).await else {
                continue;
            };
            let program = argv[0].clone();
            if !paths.iter().any(|path| path == &program) {
                continue;
            }
            if defers_to_release_handshake(&argv) {
                log_fn(&format!(
                    "{context}: {label} is running the replaced {program} and recycles itself \
                     through the installed-release handshake, so it was left to finish its slot"
                ));
                break;
            }
            let service = format!("{domain}/{label}");
            if command_stdout("/bin/launchctl", &["kickstart", "-k", &service])
                .await
                .is_some()
            {
                log_fn(&format!(
                    "{context}: restarted {service} in place; pid {pid} was running the replaced {program}"
                ));
                restarted += 1;
            } else {
                log_fn(&format!(
                    "{context}: {service} is running the replaced {program} and this account could not restart it; \
                     it keeps the old image until launchd replaces the process"
                ));
            }
            break;
        }
    }
    restarted
}

/// systemd holds a replaced image for exactly the same reason launchd does.
/// `try-restart` acts only on units that are already running and never starts
/// a stopped one, which is the in-place equivalent of `kickstart -k` here.
async fn recycle_systemd(
    context: &str,
    paths: &[String],
    ours: u32,
    log_fn: &mut dyn FnMut(&str),
) -> usize {
    let Some(listing) = command_stdout(
        "systemctl",
        &[
            "--user",
            "list-units",
            "--type=service",
            "--state=running",
            "--no-legend",
            "--plain",
        ],
    )
    .await
    else {
        log_fn(&format!(
            "{context}: systemctl --user is unavailable, so no systemd unit was restarted"
        ));
        return 0;
    };
    let mut restarted = 0usize;
    for line in listing.lines() {
        let Some(unit) = line.split_whitespace().next() else {
            continue;
        };
        let Some(exec) = command_stdout(
            "systemctl",
            &["--user", "show", "-p", "ExecStart", "--value", unit],
        )
        .await
        else {
            continue;
        };
        let Some(program) = paths.iter().find(|path| exec.contains(path.as_str())) else {
            continue;
        };
        let argv: Vec<&str> = exec.split_whitespace().collect();
        if defers_to_release_handshake(&argv) {
            log_fn(&format!(
                "{context}: {unit} is running the replaced {program} and recycles itself through \
                 the installed-release handshake, so it was left to finish its slot"
            ));
            continue;
        }
        let main_pid = command_stdout(
            "systemctl",
            &["--user", "show", "-p", "MainPID", "--value", unit],
        )
        .await
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0);
        if main_pid == ours {
            continue;
        }
        if command_stdout("systemctl", &["--user", "try-restart", unit])
            .await
            .is_some()
        {
            log_fn(&format!(
                "{context}: restarted {unit} in place; pid {main_pid} was running the replaced {program}"
            ));
            restarted += 1;
        } else {
            log_fn(&format!(
                "{context}: {unit} is running the replaced {program} and could not be restarted; \
                 it keeps the old image"
            ));
        }
    }
    restarted
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
