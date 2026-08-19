//! Local-machine install path for `stado bootstrap --local`.
//!
//! Port of `stado/deploy/local_install.py`, cut over to the Rust release
//! binaries. Picks the right init system for the host OS and writes a
//! per-user service that runs `stado agent` (for kind=local targets) or
//! `stado coordinator` (for runtime=daemon coordinators) so it persists
//! across reboots without sudo or ssh. Units ExecStart the release
//! binaries in `~/.stado/bin/` (populated from the exact immutable release
//! exposed by the public Stado API, by [`ensure_bins`] when missing)
//! and the agent unit exports WC_PYTHON so the Rust agent's Python probes
//! and job payloads use the host's job-environment interpreter.
//!
//! Darwin: launchd plist at ~/Library/LaunchAgents/<label>.plist
//!         loaded with `launchctl bootstrap gui/<uid> <plist>`.
//! Linux : systemd --user unit at ~/.config/systemd/user/<name>.service
//!         enabled with `systemctl --user enable --now <name>`.
//!
//! The plist/unit rendering is shared with `stado install-disk-cleanup`
//! (which is the `kind == "disk-cleanup"` slice of this module — see
//! `cli/disk_cleanup.rs`).

use std::fs::{self, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;

use super::{
    py_dict_repr, shlex_quote, write_if_changed, CommandOutput, CommandSpec, DeployError, Runner,
};

/// Python `LABEL_PREFIX`.
pub const LABEL_PREFIX: &str = "com.wisent.compute";

/// Fetches the central HF write token (Python `_hf_write_token`);
/// injectable so tests never touch GCS.
pub type TokenFetcher = Arc<dyn Fn() -> BoxFuture<'static, Result<String, String>> + Send + Sync>;

/// The host init system (Python `platform.system()` mapped to the two
/// supported cases).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalOs {
    Darwin,
    Linux,
}

impl LocalOs {
    /// Python: Darwin → launchd, Linux → systemd --user, anything else
    /// raises `unsupported OS for local install: {platform.system()}`.
    pub fn detect() -> Result<Self, DeployError> {
        match std::env::consts::OS {
            "macos" => Ok(Self::Darwin),
            "linux" => Ok(Self::Linux),
            other => Err(DeployError(format!(
                "unsupported OS for local install: {}",
                python_os_name(other)
            ))),
        }
    }

    /// The `platform.system()` spelling used in the dry-run header.
    pub fn python_name(&self) -> &'static str {
        match self {
            Self::Darwin => "Darwin",
            Self::Linux => "Linux",
        }
    }
}

/// `platform.system()` spelling for the unsupported-OS error.
fn python_os_name(os: &str) -> &str {
    match os {
        "macos" => "Darwin",
        "linux" => "Linux",
        "windows" => "Windows",
        other => other,
    }
}

/// Executables baked into the unit ExecStart: the release binaries under
/// `~/.stado/bin/` (populated by [`ensure_bins`] on non-dry-run installs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bins {
    pub stado: String,
    pub stado_fix: String,
    pub stado_watchdog: String,
}

impl Bins {
    /// The fixed `~/.stado/bin/` paths — stable across reinstalls so the
    /// unit's ExecStart (and therefore the capacity broadcast loop) is
    /// undisturbed by re-provisioning.
    pub fn resolve(home: &Path) -> Self {
        let bin_dir = home.join(".stado").join("bin");
        let path = |name: &str| bin_dir.join(name).to_string_lossy().into_owned();
        Self {
            stado: path("stado"),
            stado_fix: path("stado-fix"),
            stado_watchdog: path("stado-watchdog"),
        }
    }
}

/// Release binaries the local services ExecStart (stado covers agent /
/// coordinator / disk-cleanup, stado-fix the failure-fixer loop,
/// stado-watchdog the diagnostics watchdog).
pub const LOCAL_BINARIES: [&str; 3] = ["stado", "stado-fix", "stado-watchdog"];

/// Release platform dir for this host (same mapping as
/// [`crate::self_update::platform_triple_short`]).
fn release_platform() -> Result<&'static str, DeployError> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("linux", "x86_64") => Ok("linux-amd64"),
        (os, arch) => Err(DeployError(format!(
            "no release triple for platform {os}-{arch} (supported: linux-amd64, darwin-arm64)"
        ))),
    }
}

/// Populate `~/.stado/bin/` from the exact archive configured by
/// [`crate::config::stado_release_version`] when any service binary is missing.
/// The canonical manifest digest is verified before extraction or installation.
pub async fn ensure_bins(home: &Path, echo: &mut dyn FnMut(&str)) -> Result<(), DeployError> {
    ensure_bins_with(home, &crate::self_update::HttpReleaseFetcher::new(), echo).await
}

/// [`ensure_bins`] against an injected fetcher (offline tests).
pub async fn ensure_bins_with(
    home: &Path,
    fetcher: &impl crate::self_update::ReleaseFetcher,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    let version = crate::config::stado_release_version();
    ensure_bins_at_version_with(home, &version, fetcher, echo).await
}

async fn ensure_bins_at_version_with(
    home: &Path,
    version: &str,
    fetcher: &impl crate::self_update::ReleaseFetcher,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    use std::os::unix::fs::PermissionsExt;

    use crate::self_update::sha256_hex;
    let bin_dir = home.join(".stado").join("bin");
    if LOCAL_BINARIES
        .iter()
        .all(|name| bin_dir.join(name).is_file())
    {
        return Ok(());
    }
    let platform = release_platform()?;
    std::fs::create_dir_all(&bin_dir).map_err(|exc| DeployError(exc.to_string()))?;
    if version.is_empty()
        || !version
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err(DeployError(
            "STADO_RELEASE_VERSION must be an exact immutable release coordinate".to_string(),
        ));
    }
    let prefix = format!("{version}/{platform}");
    let manifest_name = format!("release-manifest-{platform}.json");
    let manifest_bytes = fetcher
        .fetch(&format!("{prefix}/{manifest_name}"))
        .await
        .map_err(|exc| {
            DeployError(format!(
                "release download failed for {manifest_name}: {exc}"
            ))
        })?
        .ok_or_else(|| DeployError(format!("{manifest_name} is not published")))?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| DeployError(format!("invalid release manifest: {error}")))?;
    let object = manifest
        .as_object()
        .ok_or_else(|| DeployError("release manifest must be an object".to_string()))?;
    if object.len() != 5
        || manifest.get("product").and_then(Value::as_str) != Some("stado")
        || manifest.get("version").and_then(Value::as_str) != Some(version)
        || manifest.get("platform").and_then(Value::as_str) != Some(platform)
        || !manifest
            .get("source_commit")
            .and_then(Value::as_str)
            .is_some_and(|commit| {
                matches!(commit.len(), 40 | 64)
                    && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
    {
        return Err(DeployError(
            "release manifest identity is invalid".to_string(),
        ));
    }
    let expected = manifest
        .get("sha256")
        .and_then(Value::as_str)
        .filter(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| DeployError("release manifest sha256 is invalid".to_string()))?;
    let archive_name = format!("stado-v{version}-{platform}.tar.gz");
    let archive = fetcher
        .fetch(&format!("{prefix}/{archive_name}"))
        .await
        .map_err(|exc| DeployError(format!("release download failed for {archive_name}: {exc}")))?
        .ok_or_else(|| DeployError(format!("{archive_name} is not published")))?;
    let actual = sha256_hex(&archive);
    if actual != expected {
        return Err(DeployError(format!(
            "sha256 mismatch for {archive_name}: expected {expected}, got {actual}"
        )));
    }
    let staging = tempfile::tempdir().map_err(|error| DeployError(error.to_string()))?;
    let extracted = staging.path().join("archive");
    crate::release_control::safe_extract_archive(&archive, &extracted).map_err(DeployError)?;
    let mut verified: Vec<(&str, Vec<u8>)> = Vec::with_capacity(LOCAL_BINARIES.len());
    for name in LOCAL_BINARIES {
        let path = extracted.join(name);
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| DeployError(error.to_string()))?;
        if !metadata.file_type().is_file() || metadata.len() == 0 {
            return Err(DeployError(format!(
                "release archive member {name} is not a non-empty regular file"
            )));
        }
        let bytes = std::fs::read(path).map_err(|error| DeployError(error.to_string()))?;
        verified.push((name, bytes));
    }
    for (name, bytes) in verified {
        let dest = bin_dir.join(name);
        std::fs::write(&dest, &bytes).map_err(|exc| DeployError(exc.to_string()))?;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
            .map_err(|exc| DeployError(exc.to_string()))?;
    }
    echo(&format!(
        "[install] downloaded stado {} ({platform}) -> {}",
        version,
        bin_dir.display()
    ));
    Ok(())
}

/// Python: `f"{LABEL_PREFIX}.{kind}.{entry.name}"`.
pub fn label(kind: &str, name: &str) -> String {
    format!("{LABEL_PREFIX}.{kind}.{name}")
}

fn local_control_plane_configured() -> bool {
    crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
        == Some(crate::capabilities::StorageAdapter::Local)
        && crate::config::wc_providers()
            .iter()
            .all(|provider| crate::capabilities::ProviderId::Local.matches(provider))
}

/// Python `_exec_args_for(entry, kind)`.
pub fn exec_args_for(bins: &Bins, kind: &str, _name: &str) -> Result<Vec<String>, DeployError> {
    match kind {
        "agent" => Ok(vec![
            bins.stado.clone(),
            "agent".to_string(),
            "--auto".to_string(),
        ]),
        "coordinator" if local_control_plane_configured() => {
            Ok(vec![bins.stado.clone(), "local-control-plane".to_string()])
        }
        "coordinator" => Ok(vec![
            bins.stado.clone(),
            "cloud-control-plane".to_string(),
            "--bind".to_string(),
            crate::config::dashboard_bind().to_string(),
            "--port".to_string(),
            crate::config::dashboard_port().to_string(),
        ]),
        "disk-cleanup" => Ok(vec![
            bins.stado.clone(),
            "disk-cleanup".to_string(),
            "--watch".to_string(),
        ]),
        "failure-fixer" => {
            // Run scan_and_dispatch every iteration. Loop in shell so a
            // single failure of scan_and_dispatch (transient GCS hiccup,
            // model-router 5xx) does not require launchd to restart the
            // whole job; the next iteration retries cleanly.
            let pattern = crate::config::FAILURE_FIXER_COMMAND_PATTERN;
            let pat_arg = if pattern.is_empty() {
                String::new()
            } else {
                format!("--command-pattern '{pattern}'")
            };
            Ok(vec![
                "/bin/bash".to_string(),
                "-c".to_string(),
                format!(
                    "while true; do {} scan-dispatch --execute {pat_arg}; sleep {}; done",
                    bins.stado_fix,
                    crate::config::FAILURE_FIXER_TICK_SECONDS
                ),
            ])
        }
        "watchdog" => Ok(vec![bins.stado_watchdog.clone()]),
        other => Err(DeployError(format!("unknown install kind: {other}"))),
    }
}

/// The python.org 3.12 framework interpreter the fleet's mac minis install
/// the job environment (wisent, transformers, ...) into.
pub const FRAMEWORK_PYTHON: &str =
    "/Library/Frameworks/Python.framework/Versions/3.12/bin/python3.12";

/// The WC_PYTHON value baked into agent units: the Rust agent's Python
/// probes (smoketest, CUDA probe, fleet flush) and the job payloads still
/// run as Python, so the unit must point at the interpreter that has the
/// job environment installed. Operator override via $WC_PYTHON first, then
/// [`FRAMEWORK_PYTHON`] when present, else plain `python3`.
pub fn default_wc_python() -> String {
    if let Ok(value) = std::env::var("WC_PYTHON") {
        if !value.trim().is_empty() {
            return value;
        }
    }
    if Path::new(FRAMEWORK_PYTHON).is_file() {
        return FRAMEWORK_PYTHON.to_string();
    }
    "python3".to_string()
}

/// Central write-scoped Hugging Face token from
/// `stado-huggingface/write_token` in Skarbiec. Missing credentials,
/// authorization and transport failures are explicit; there is no alternate
/// credential source.
pub async fn fetch_hf_write_token() -> Result<String, DeployError> {
    crate::skarbiec::read_string("stado-huggingface", "write_token")
        .await
        .map_err(|exc| DeployError(exc.to_string()))?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            DeployError("Skarbiec item stado-huggingface field write_token is required".into())
        })
}

/// Production [`TokenFetcher`] over [`fetch_hf_write_token`].
pub fn production_hf_fetcher() -> TokenFetcher {
    Arc::new(|| Box::pin(async { fetch_hf_write_token().await.map_err(|exc| exc.0) }))
}

/// Explicit inputs used by the provider-neutral unit renderer.
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvInputs<'a> {
    pub wc_python: &'a str,
    pub path: Option<&'a str>,
}

/// The unit environment, in Python dict insertion order (the plist/unit
/// renderers iterate this order byte-exactly).
pub fn build_env(kind: &str, inputs: &EnvInputs) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vec![("PYTHONUNBUFFERED".to_string(), "1".to_string())];
    let agent_url = crate::config::agent_skarbiec_url();
    let (skarbiec_url, skarbiec_consumer, skarbiec_token_file) = if kind == "agent" {
        (
            if agent_url.is_empty() {
                crate::config::skarbiec_url()
            } else {
                agent_url
            },
            crate::config::agent_skarbiec_consumer(),
            crate::config::agent_skarbiec_token_file(),
        )
    } else {
        (
            crate::config::skarbiec_url(),
            crate::config::skarbiec_consumer(),
            crate::config::skarbiec_token_file(),
        )
    };
    env.push(("WC_SKARBIEC_URL".to_string(), skarbiec_url.to_string()));
    env.push((
        "WC_SKARBIEC_CONSUMER".to_string(),
        skarbiec_consumer.to_string(),
    ));
    env.push((
        "WC_SKARBIEC_TOKEN_FILE".to_string(),
        skarbiec_token_file.to_string(),
    ));
    if kind == "agent" {
        env.push((
            "WC_AGENT_SKARBIEC_URL".to_string(),
            skarbiec_url.to_string(),
        ));
        env.push((
            "WC_AGENT_SKARBIEC_CONSUMER".to_string(),
            skarbiec_consumer.to_string(),
        ));
        env.push((
            "WC_AGENT_SKARBIEC_TOKEN_FILE".to_string(),
            skarbiec_token_file.to_string(),
        ));
        env.push((
            "WC_AGENT_SKARBIEC_ITEMS".to_string(),
            crate::config::agent_skarbiec_items().join(","),
        ));
        env.push((
            "WC_AGENT_SKARBIEC_SECRET_FIELDS".to_string(),
            crate::config::agent_skarbiec_secret_fields().join(","),
        ));
    }
    // The standalone agent and the outage-safe local control plane both
    // execute Python probes and job payloads. Preserve the operator PATH so
    // child jobs see the same toolchain as an interactive Stado invocation.
    let runs_local_agent =
        kind == "agent" || (kind == "coordinator" && local_control_plane_configured());
    if runs_local_agent {
        if !inputs.wc_python.is_empty() {
            env.push(("WC_PYTHON".to_string(), inputs.wc_python.to_string()));
        }
        let path = inputs
            .path
            .unwrap_or("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin");
        env.push(("PATH".to_string(), path.to_string()));
    }
    // Failure-fixer and watchdog resolve credentials and backend routing
    // through Stado config and Skarbiec. Only PATH is inherited here.
    if kind == "failure-fixer" || kind == "watchdog" {
        let path = inputs
            .path
            .unwrap_or("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin");
        env.push(("PATH".to_string(), path.to_string()));
    }
    env
}

/// [`build_env`] with the provider-neutral process inputs used by installed
/// services. Backend routing comes only from `STADO_CONFIG`.
pub fn install_env(
    _home: &Path,
    kind: &str,
    _hf_token: &str,
    wc_python: &str,
) -> Vec<(String, String)> {
    let path = std::env::var("PATH").ok();
    let inputs = EnvInputs {
        wc_python,
        path: path.as_deref(),
    };
    let mut env = build_env(kind, &inputs);
    if let Ok(Some(path)) = crate::config_file::config_path() {
        env.push((
            "STADO_CONFIG".to_string(),
            path.to_string_lossy().into_owned(),
        ));
    }
    let deployment_id = crate::config::stado_deployment_id();
    if !deployment_id.is_empty() {
        env.push(("STADO_DEPLOYMENT_ID".to_string(), deployment_id));
    }
    env
}

/// Render a launchd agent plist with an explicit owner-controlled log path.
pub fn plist_text(
    label: &str,
    exec_args: &[String],
    env: &[(String, String)],
    log: &Path,
) -> String {
    plist_document(label, exec_args, env, log, None)
}

/// The same job rendered for launchd's **system** domain, running as `user`.
///
/// The per-user domain does not exist on an ssh login with no Aqua session:
/// `launchctl bootstrap gui/$uid` answers `Could not switch to audit session`
/// and `stado service deploy` came back having installed nothing, which is how
/// two `stado agent` processes ran for four days with no unit behind them. A
/// daemon in `/Library/LaunchDaemons` is the domain that does exist over ssh,
/// and `UserName` is what keeps the process out of root: without it launchd
/// would run the fleet's own control binary as uid 0 against an account-owned
/// `~/.stado`.
pub fn daemon_plist_text(
    label: &str,
    exec_args: &[String],
    env: &[(String, String)],
    log: &Path,
    user: &str,
) -> String {
    plist_document(label, exec_args, env, log, Some(user))
}

/// One renderer for both domains, so an agent and the daemon spelling of the
/// same unit cannot come to disagree about anything but the account.
fn plist_document(
    label: &str,
    exec_args: &[String],
    env: &[(String, String)],
    log: &Path,
    user: Option<&str>,
) -> String {
    let user_xml = match user {
        Some(user) => format!("    <key>UserName</key>\n    <string>{user}</string>\n"),
        None => String::new(),
    };
    let args_xml: String = exec_args
        .iter()
        .map(|a| format!("        <string>{a}</string>\n"))
        .collect();
    let env_xml: String = env
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("        <key>{k}</key>\n        <string>{v}</string>\n"))
        .collect();
    let log = log.to_string_lossy();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
{args_xml}    </array>
{user_xml}    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
    <key>EnvironmentVariables</key>
    <dict>
{env_xml}    </dict>
</dict>
</plist>
"#
    )
}

/// Python `_systemd_user_unit`.
pub fn systemd_user_unit(
    description: &str,
    exec_args: &[String],
    env: &[(String, String)],
) -> String {
    let env_lines: String = env
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("Environment={k}={v}\n"))
        .collect();
    let cmd = exec_args.join(" ");
    format!(
        "[Unit]\nDescription={description}\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\n{env_lines}ExecStart={cmd}\nRestart=on-failure\nRestartSec=30\n\n[Install]\nWantedBy=default.target\n"
    )
}

/// Everything needed to install one local service: the pure plan Python
/// computes inside `install_local` before touching the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub name: String,
    pub kind: String,
    pub os: LocalOs,
    pub label: String,
    pub exec_args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl InstallPlan {
    /// Python `f"Wisent Compute {kind} ({entry.name})"` (Linux unit
    /// Description).
    pub fn description(&self) -> String {
        format!("Wisent Compute {} ({})", self.kind, self.name)
    }

    /// The plist (Darwin) or unit (Linux) destination path under `home`.
    pub fn unit_path(&self, home: &Path) -> PathBuf {
        match self.os {
            LocalOs::Darwin => home
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{}.plist", self.label)),
            LocalOs::Linux => home
                .join(".config")
                .join("systemd")
                .join("user")
                .join(format!("{}.service", self.label)),
        }
    }

    /// The plist (Darwin) or unit (Linux) content for an account home.
    pub fn content(&self, home: &Path) -> String {
        match self.os {
            LocalOs::Darwin => plist_text(
                &self.label,
                &self.exec_args,
                &self.env,
                &home
                    .join(".stado")
                    .join("logs")
                    .join(format!("{}.log", self.label)),
            ),
            LocalOs::Linux => systemd_user_unit(&self.description(), &self.exec_args, &self.env),
        }
    }

    /// Python's dry-run branch of `install_local`.
    pub fn dry_run_lines(&self) -> Vec<String> {
        vec![
            format!(
                "[dry-run] {}={} on {}",
                self.kind,
                self.name,
                self.os.python_name()
            ),
            format!("  exec: {}", self.exec_args.join(" ")),
            format!("  env:  {}", py_dict_repr(&self.env)),
        ]
    }
}

/// Build the [`InstallPlan`] for one (name, kind) pair.
pub fn plan(
    name: &str,
    kind: &str,
    os: LocalOs,
    home: &Path,
    bins: &Bins,
    hf_token: &str,
    wc_python: &str,
) -> Result<InstallPlan, DeployError> {
    Ok(InstallPlan {
        name: name.to_string(),
        kind: kind.to_string(),
        os,
        label: label(kind, name),
        exec_args: exec_args_for(bins, kind, name)?,
        env: install_env(home, kind, hf_token, wc_python),
    })
}

/// Python `_install_darwin` launchctl argv: (bootout, bootstrap, kickstart).
pub fn darwin_commands(label: &str, plist_path: &Path, uid: u32) -> [CommandSpec; 3] {
    [
        CommandSpec::new(vec![
            "launchctl".to_string(),
            "bootout".to_string(),
            format!("gui/{uid}/{label}"),
        ]),
        CommandSpec::new(vec![
            "launchctl".to_string(),
            "bootstrap".to_string(),
            format!("gui/{uid}"),
            plist_path.to_string_lossy().into_owned(),
        ]),
        CommandSpec::new(vec![
            "launchctl".to_string(),
            "kickstart".to_string(),
            "-k".to_string(),
            format!("gui/{uid}/{label}"),
        ]),
    ]
}

/// Python `_install_linux` systemctl argv: (daemon-reload, enable --now).
pub fn linux_commands(label: &str) -> [CommandSpec; 2] {
    [
        CommandSpec::new(vec![
            "systemctl".to_string(),
            "--user".to_string(),
            "daemon-reload".to_string(),
        ]),
        CommandSpec::new(vec![
            "systemctl".to_string(),
            "--user".to_string(),
            "enable".to_string(),
            "--now".to_string(),
            format!("{label}.service"),
        ]),
    ]
}

/// The uid for `gui/<uid>` launchd domains (Python `os.getuid()`).
pub fn current_uid() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { nix::libc::getuid() }
}
fn prepare_owner_log(home: &Path, label: &str) -> Result<PathBuf, DeployError> {
    let directory = home.join(".stado").join("logs");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(DeployError(format!(
                "refusing non-directory agent log path {}",
                directory.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&directory).map_err(|error| DeployError(error.to_string()))?;
        }
        Err(error) => return Err(DeployError(error.to_string())),
    }
    #[allow(clippy::unnecessary_cast)] // mode_t is u16 on macOS, u32 on Linux
    let directory_mode = nix::libc::S_IRWXU as u32;
    fs::set_permissions(&directory, fs::Permissions::from_mode(directory_mode))
        .map_err(|error| DeployError(error.to_string()))?;

    let log = directory.join(format!("{label}.log"));
    match fs::symlink_metadata(&log) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(DeployError(format!(
                "refusing non-file agent log path {}",
                log.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(DeployError(error.to_string())),
    }
    #[allow(clippy::unnecessary_cast)] // mode_t is u16 on macOS, u32 on Linux
    let file_mode = (nix::libc::S_IRUSR | nix::libc::S_IWUSR) as u32;
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(file_mode)
        .open(&log)
        .map_err(|error| DeployError(error.to_string()))?;
    fs::set_permissions(&log, fs::Permissions::from_mode(file_mode))
        .map_err(|error| DeployError(error.to_string()))?;
    Ok(log)
}

/// Persistent headless-mac fallback when launchctl refuses bootstrap from
/// an SSH audit session. Cron starts the same generated environment on boot,
/// and nohup starts it immediately.
async fn install_cron_fallback(
    plan: &InstallPlan,
    home: &Path,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    let wrapper = home
        .join(".stado")
        .join("bin")
        .join(format!("run-{}.sh", plan.label));
    let log = prepare_owner_log(home, &plan.label)?;
    let mut content = String::from("#!/bin/sh\n");
    for (key, value) in &plan.env {
        if !value.is_empty() {
            content.push_str(&format!("export {key}={}\n", shlex_quote(value)));
        }
    }
    content.push_str("exec");
    for arg in &plan.exec_args {
        content.push(' ');
        content.push_str(&shlex_quote(arg));
    }
    content.push('\n');
    write_if_changed(&wrapper, &content).map_err(|exc| DeployError(exc.to_string()))?;

    let wrapper_arg = shlex_quote(&wrapper.to_string_lossy());
    let cron_line = format!(
        "@reboot /bin/sh {wrapper_arg} >> {} 2>&1",
        shlex_quote(&log.to_string_lossy())
    );
    let cron_script = format!(
        "{{ crontab -l 2>/dev/null | grep -Fv -- {wrapper_arg} || true; printf '%s\\n' {}; }} | crontab -",
        shlex_quote(&cron_line)
    );
    let cron = runner(CommandSpec::new(vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        cron_script,
    ]))
    .await
    .map_err(DeployError)?;
    if !cron.ok() {
        return Err(DeployError(format!(
            "crontab install failed: {}",
            cron.detail()
        )));
    }
    let start = runner(CommandSpec::new(vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!(
            "nohup /bin/sh {wrapper_arg} >> {} 2>&1 </dev/null &",
            shlex_quote(&log.to_string_lossy())
        ),
    ]))
    .await
    .map_err(DeployError)?;
    if !start.ok() {
        return Err(DeployError(format!(
            "agent start failed: {}",
            start.detail()
        )));
    }
    echo(&format!(
        "[ok]   installed headless cron job {} (logs: {})",
        plan.label,
        log.display()
    ));
    Ok(())
}

/// Execute an [`InstallPlan`] (Python `_install_darwin` / `_install_linux`):
/// write the file (skipping a byte-identical rewrite), then boot the job.
pub async fn execute_plan(
    plan: &InstallPlan,
    home: &Path,
    uid: u32,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    let path = plan.unit_path(home);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|exc| DeployError(exc.to_string()))?;
    }
    let log = if plan.os == LocalOs::Darwin {
        Some(prepare_owner_log(home, &plan.label)?)
    } else {
        None
    };
    let written =
        write_if_changed(&path, &plan.content(home)).map_err(|exc| DeployError(exc.to_string()))?;
    let verb = if written { "wrote" } else { "unchanged" };
    match plan.os {
        LocalOs::Darwin => {
            echo(&format!("[plist] {verb} {}", path.display()));
            let [bootout, bootstrap, kickstart] = darwin_commands(&plan.label, &path, uid);
            let _ = runner(bootout).await.map_err(DeployError)?;
            // Retry bootstrap: launchd sporadically rejects a fresh domain
            // right after bootout (Python: 5 attempts, 0.5s apart).
            let mut last: Option<CommandOutput> = None;
            for attempt in 0..5 {
                let output = runner(bootstrap.clone()).await.map_err(DeployError)?;
                let ok = output.ok();
                last = Some(output);
                if ok {
                    break;
                }
                if attempt < 4 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
            let Some(output) = last else {
                return Err("launchctl did not run".into());
            };
            if output.ok() {
                let _ = runner(kickstart).await.map_err(DeployError)?;
            } else {
                // Headless SSH sessions can see the logged-in user's domain
                // while macOS rejects bootstrap actions against its gui alias.
                let domain = format!("user/{uid}");
                let service = format!("{domain}/{}", plan.label);
                let _ = runner(CommandSpec::new(vec![
                    "launchctl".to_string(),
                    "bootout".to_string(),
                    service.clone(),
                ]))
                .await
                .map_err(DeployError)?;
                let fallback = runner(CommandSpec::new(vec![
                    "launchctl".to_string(),
                    "bootstrap".to_string(),
                    domain.clone(),
                    path.to_string_lossy().into_owned(),
                ]))
                .await
                .map_err(DeployError)?;
                if fallback.ok() {
                    let _ = runner(CommandSpec::new(vec![
                        "launchctl".to_string(),
                        "kickstart".to_string(),
                        "-k".to_string(),
                        service,
                    ]))
                    .await
                    .map_err(DeployError)?;
                } else {
                    let gui_domain = format!("gui/{uid}");
                    let gui_service = format!("{gui_domain}/{}", plan.label);
                    let asuser = uid.to_string();
                    let contextual = runner(CommandSpec::new(vec![
                        "launchctl".to_string(),
                        "asuser".to_string(),
                        asuser.clone(),
                        "launchctl".to_string(),
                        "bootstrap".to_string(),
                        gui_domain,
                        path.to_string_lossy().into_owned(),
                    ]))
                    .await
                    .map_err(DeployError)?;
                    if !contextual.ok() {
                        echo(
                            "[warn] launchctl unavailable in this SSH session; using cron fallback",
                        );
                        install_cron_fallback(plan, home, runner, echo).await?;
                        return Ok(());
                    }
                    let _ = runner(CommandSpec::new(vec![
                        "launchctl".to_string(),
                        "asuser".to_string(),
                        asuser,
                        "launchctl".to_string(),
                        "kickstart".to_string(),
                        "-k".to_string(),
                        gui_service,
                    ]))
                    .await
                    .map_err(DeployError)?;
                }
            }
            let Some(log) = log.as_ref() else {
                return Err(DeployError("launchd log path was not prepared".to_string()));
            };
            echo(&format!(
                "[ok]   loaded launchd job {} (logs: {})",
                plan.label,
                log.display()
            ));
        }
        LocalOs::Linux => {
            echo(&format!("[unit] {verb} {}", path.display()));
            let [daemon_reload, enable] = linux_commands(&plan.label);
            let _ = runner(daemon_reload).await.map_err(DeployError)?;
            let output = runner(enable).await.map_err(DeployError)?;
            if !output.ok() {
                return Err(DeployError(format!(
                    "systemctl enable failed: {}",
                    output.detail()
                )));
            }
            echo(&format!("[ok]   enabled systemd --user job {}", plan.label));
        }
    }
    Ok(())
}

/// Install a persistent local service. Credentials remain in Skarbiec; the
/// service unit receives only Skarbiec connection metadata and non-secret
/// runtime configuration.
pub async fn install_local(
    name: &str,
    kind: &str,
    dry_run: bool,
    runner: &Runner,
    _hf_fetch: &TokenFetcher,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    let os = LocalOs::detect()?;
    let home = crate::config_file::expand_tilde("~");
    let bins = Bins::resolve(&home);
    let wc_python = default_wc_python();
    let install_plan = plan(name, kind, os, &home, &bins, "", &wc_python)?;
    if dry_run {
        for line in install_plan.dry_run_lines() {
            echo(&line);
        }
        return Ok(());
    }
    ensure_bins(&home, echo).await?;
    execute_plan(&install_plan, &home, current_uid(), runner, echo).await
}

