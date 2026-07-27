//! Local-machine install path for `stado bootstrap --local`.
//!
//! Port of `stado/deploy/local_install.py`, cut over to the Rust release
//! binaries. Picks the right init system for the host OS and writes a
//! per-user service that runs `stado agent` (for kind=local targets) or
//! `stado coordinator` (for runtime=daemon coordinators) so it persists
//! across reboots without sudo or ssh. Units ExecStart the release
//! binaries in `~/.stado/bin/` (populated from the release channel,
//! [`crate::config::release_base_url`], by [`ensure_bins`] when missing)
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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

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

/// Populate `~/.stado/bin/` from the release channel
/// ([`crate::config::release_base_url`]) when any service binary is
/// missing. Every binary is checksum-verified against the release's
/// SHA256SUMS before anything is installed; a fully populated dir is a
/// no-op.
///
/// Fetched over plain HTTPS rather than shelled out to `gcloud storage
/// cp`: the box being provisioned is exactly the box that may have
/// neither gcloud nor GCP credentials, and gcloud cannot read a release
/// channel hosted anywhere but GCS.
pub async fn ensure_bins(home: &Path, echo: &mut dyn FnMut(&str)) -> Result<(), DeployError> {
    ensure_bins_with(home, &crate::self_update::HttpReleaseFetcher::new(), echo).await
}

/// [`ensure_bins`] against an injected fetcher (offline tests).
pub async fn ensure_bins_with(
    home: &Path,
    fetcher: &impl crate::self_update::ReleaseFetcher,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    use std::os::unix::fs::PermissionsExt;

    use crate::self_update::{
        parse_latest_json, parse_sha256sums, sha256_hex, LATEST_JSON_NAME, SHA256SUMS_NAME,
    };
    let bin_dir = home.join(".stado").join("bin");
    if LOCAL_BINARIES
        .iter()
        .all(|name| bin_dir.join(name).is_file())
    {
        return Ok(());
    }
    let platform = release_platform()?;
    std::fs::create_dir_all(&bin_dir).map_err(|exc| DeployError(exc.to_string()))?;
    let latest = fetcher
        .fetch(LATEST_JSON_NAME)
        .await
        .map_err(|exc| DeployError(format!("release lookup failed: {exc}")))?
        .ok_or_else(|| {
            DeployError(format!(
                "release lookup failed: {LATEST_JSON_NAME} is missing"
            ))
        })?;
    let latest = String::from_utf8(latest)
        .map_err(|exc| DeployError(format!("release lookup failed: {exc}")))?;
    let release = parse_latest_json(latest.trim())
        .ok_or_else(|| DeployError(format!("malformed {LATEST_JSON_NAME}: {}", latest.trim())))?;
    let prefix = format!("{}/{platform}", release.version);
    let sums_bytes = fetcher
        .fetch(&format!("{prefix}/{SHA256SUMS_NAME}"))
        .await
        .map_err(|exc| {
            DeployError(format!(
                "release download failed for {SHA256SUMS_NAME}: {exc}"
            ))
        })?
        .ok_or_else(|| {
            DeployError(format!(
                "release download failed for {SHA256SUMS_NAME}: not published"
            ))
        })?;
    let sums = parse_sha256sums(
        std::str::from_utf8(&sums_bytes).map_err(|exc| DeployError(exc.to_string()))?,
    )
    .map_err(|exc| DeployError(exc.to_string()))?;
    // Download + verify EVERY binary before installing the first
    // (self_update's abort-before-first-rename stance): a bad hash leaves
    // ~/.stado/bin untouched.
    let mut verified: Vec<(&str, Vec<u8>)> = Vec::with_capacity(LOCAL_BINARIES.len());
    for name in LOCAL_BINARIES {
        let bytes = fetcher
            .fetch(&format!("{prefix}/{name}"))
            .await
            .map_err(|exc| DeployError(format!("release download failed for {name}: {exc}")))?
            .ok_or_else(|| {
                DeployError(format!("release download failed for {name}: not published"))
            })?;
        let expected = sums
            .get(name)
            .ok_or_else(|| DeployError(format!("SHA256SUMS has no entry for {name}")))?;
        let actual = sha256_hex(&bytes);
        if actual != *expected {
            return Err(DeployError(format!(
                "sha256 mismatch for {name}: expected {expected}, got {actual}"
            )));
        }
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
        release.version,
        bin_dir.display()
    ));
    Ok(())
}

/// Python: `f"{LABEL_PREFIX}.{kind}.{entry.name}"`.
pub fn label(kind: &str, name: &str) -> String {
    format!("{LABEL_PREFIX}.{kind}.{name}")
}

/// Python `_exec_args_for(entry, kind)`.
pub fn exec_args_for(bins: &Bins, kind: &str, _name: &str) -> Result<Vec<String>, DeployError> {
    match kind {
        "agent" => Ok(vec![
            bins.stado.clone(),
            "agent".to_string(),
            "--auto".to_string(),
        ]),
        "coordinator"
            if crate::config::wc_storage_backend() == "local"
                && crate::config::wc_providers()
                    .iter()
                    .all(|provider| provider == "local") =>
        {
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

/// Path of the Skarbiec-materialized service-account transport file, if one
/// exists. GCP managed-identity hosts intentionally have no such file.
pub fn adc_path(home: &Path) -> String {
    let path = home.join(".config/gcloud/application_default_credentials.json");
    path.is_file()
        .then(|| path.to_string_lossy().into_owned())
        .unwrap_or_default()
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

/// Process-environment inputs to [`build_env`] (Python's `os.environ`
/// reads), separated so tests never depend on the developer's env.
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvInputs<'a> {
    pub adc: &'a str,
    pub hf_token: &'a str,
    pub wc_python: &'a str,
    pub path: Option<&'a str>,
    pub google_cloud_project: Option<&'a str>,
    pub gcp_project: Option<&'a str>,
    pub wc_bucket: Option<&'a str>,
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.is_empty())
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
        env.push(("WC_AGENT_SKARBIEC_URL".to_string(), skarbiec_url.to_string()));
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
    let runs_local_agent = kind == "agent"
        || (kind == "coordinator"
            && crate::config::wc_storage_backend() == "local"
            && crate::config::wc_providers()
                .iter()
                .all(|provider| provider == "local"));
    if runs_local_agent {
        if !inputs.wc_python.is_empty() {
            env.push(("WC_PYTHON".to_string(), inputs.wc_python.to_string()));
        }
        let path = inputs
            .path
            .unwrap_or("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin");
        env.push(("PATH".to_string(), path.to_string()));
    }
    if kind == "disk-cleanup" {
        let project = non_empty(inputs.google_cloud_project)
            .or(non_empty(inputs.gcp_project))
            .unwrap_or_else(|| crate::config::project());
        env.push(("GOOGLE_CLOUD_PROJECT".to_string(), project.to_string()));
    }
    // Failure-fixer receives its Anthropic credential directly from Skarbiec
    // when spawning Claude. PATH locates the executable; project and bucket
    // values are non-secret routing configuration used by maintenance jobs.
    if kind == "failure-fixer" || kind == "watchdog" {
        let path = inputs
            .path
            .unwrap_or("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin");
        env.push(("PATH".to_string(), path.to_string()));
        if let Some(v) = non_empty(inputs.google_cloud_project) {
            env.push(("GOOGLE_CLOUD_PROJECT".to_string(), v.to_string()));
        }
        if let Some(v) = non_empty(inputs.gcp_project) {
            env.push(("GCP_PROJECT".to_string(), v.to_string()));
        }
        let bucket = non_empty(inputs.wc_bucket).unwrap_or("wisent-compute");
        env.push(("WC_BUCKET".to_string(), bucket.to_string()));
    }
    env
}

/// [`build_env`] with inputs probed from the process environment and the
/// filesystem (Python's direct `os.environ` / `Path.home()` reads).
///
/// Skarbiec routing metadata is included in every installed service.
/// `adc` remains an input only for renderer compatibility and is never
/// exported as a credential source.
pub fn install_env(
    home: &Path,
    kind: &str,
    hf_token: &str,
    wc_python: &str,
) -> Vec<(String, String)> {
    let adc = if crate::config::wc_storage_backend() == "gcs" {
        adc_path(home)
    } else {
        String::new()
    };
    let path = std::env::var("PATH").ok();
    let google_cloud_project = std::env::var("GOOGLE_CLOUD_PROJECT").ok();
    let gcp_project = std::env::var("GCP_PROJECT").ok();
    let wc_bucket = std::env::var("WC_BUCKET").ok();
    let inputs = EnvInputs {
        adc: &adc,
        hf_token,
        wc_python,
        path: path.as_deref(),
        google_cloud_project: google_cloud_project.as_deref(),
        gcp_project: gcp_project.as_deref(),
        wc_bucket: wc_bucket.as_deref(),
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

/// Python `_plist_text` (raw interpolation, exactly like the Python — no
/// XML escaping, same as the source).
pub fn plist_text(label: &str, exec_args: &[String], env: &[(String, String)]) -> String {
    let args_xml: String = exec_args
        .iter()
        .map(|a| format!("        <string>{a}</string>\n"))
        .collect();
    let env_xml: String = env
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("        <key>{k}</key>\n        <string>{v}</string>\n"))
        .collect();
    let log = format!("/tmp/{label}.log");
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
    <key>RunAtLoad</key>
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

    /// The file content to install (byte-exact with the Python renderers).
    pub fn content(&self) -> String {
        match self.os {
            LocalOs::Darwin => plist_text(&self.label, &self.exec_args, &self.env),
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
    let log = format!("/tmp/{}.log", plan.label);
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
        shlex_quote(&log)
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
            shlex_quote(&log)
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
        "[ok]   installed headless cron job {} (logs: {log})",
        plan.label
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
    let written =
        write_if_changed(&path, &plan.content()).map_err(|exc| DeployError(exc.to_string()))?;
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
            echo(&format!(
                "[ok]   loaded launchd job {} (logs: /tmp/{}.log)",
                plan.label, plan.label
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn bins() -> Bins {
        Bins {
            stado: "/Users/u/.stado/bin/stado".to_string(),
            stado_fix: "/Users/u/.stado/bin/stado-fix".to_string(),
            stado_watchdog: "/Users/u/.stado/bin/stado-watchdog".to_string(),
        }
    }

    fn fake_runner(outputs: Vec<CommandOutput>) -> (Runner, Arc<Mutex<Vec<CommandSpec>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(Mutex::new(outputs));
        let calls2 = Arc::clone(&calls);
        let runner = super::super::runner_fn(move |spec| {
            let calls = Arc::clone(&calls2);
            let queue = Arc::clone(&queue);
            async move {
                calls.lock().unwrap().push(spec);
                Ok(queue.lock().unwrap().remove(0))
            }
        });
        (runner, calls)
    }

    fn out(code: i32, stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn label_matches_python() {
        assert_eq!(
            label("disk-cleanup", "disk-cleanup"),
            "com.wisent.compute.disk-cleanup.disk-cleanup"
        );
        assert_eq!(
            label("agent", "mini-one"),
            "com.wisent.compute.agent.mini-one"
        );
    }

    #[test]
    fn exec_args_for_all_kinds() {
        let bins = bins();
        assert_eq!(
            exec_args_for(&bins, "agent", "mini-one").unwrap(),
            vec!["/Users/u/.stado/bin/stado", "agent", "--auto"]
        );
        assert_eq!(
            exec_args_for(&bins, "coordinator", "main").unwrap(),
            vec![
                "/Users/u/.stado/bin/stado",
                "coordinator",
                "--target",
                "main"
            ]
        );
        assert_eq!(
            exec_args_for(&bins, "disk-cleanup", "disk-cleanup").unwrap(),
            vec!["/Users/u/.stado/bin/stado", "disk-cleanup", "--watch"]
        );
        assert_eq!(
            exec_args_for(&bins, "failure-fixer", "failure-fixer").unwrap(),
            vec![
                "/bin/bash",
                "-c",
                "while true; do /Users/u/.stado/bin/stado-fix scan-dispatch --execute --command-pattern 'raw.extract_and_upload'; sleep 180; done",
            ]
        );
        assert_eq!(
            exec_args_for(&bins, "watchdog", "watchdog").unwrap(),
            vec!["/Users/u/.stado/bin/stado-watchdog"]
        );
        let err = exec_args_for(&bins, "bogus", "x").unwrap_err();
        assert_eq!(err.0, "unknown install kind: bogus");
    }

    #[test]
    fn bins_resolve_points_at_home_stado_bin() {
        let home = Path::new("/home/u");
        let bins = Bins::resolve(home);
        assert_eq!(bins.stado, "/home/u/.stado/bin/stado");
        assert_eq!(bins.stado_fix, "/home/u/.stado/bin/stado-fix");
        assert_eq!(bins.stado_watchdog, "/home/u/.stado/bin/stado-watchdog");
    }

    #[test]
    fn build_env_agent_order_and_hf_token() {
        let env = build_env(
            "agent",
            &EnvInputs {
                adc: "/home/u/adc.json",
                hf_token: "hf_secret",
                wc_python: FRAMEWORK_PYTHON,
                ..Default::default()
            },
        );
        assert_eq!(
            env,
            vec![
                ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
                (
                    "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
                    "/home/u/adc.json".to_string()
                ),
                ("WC_PYTHON".to_string(), FRAMEWORK_PYTHON.to_string()),
                ("HF_TOKEN".to_string(), "hf_secret".to_string()),
                (
                    "HUGGING_FACE_HUB_TOKEN".to_string(),
                    "hf_secret".to_string()
                ),
            ]
        );
        // Missing token (blob absent) leaves no HF keys at all.
        let env = build_env("agent", &EnvInputs::default());
        assert_eq!(env, vec![("PYTHONUNBUFFERED".to_string(), "1".to_string())]);
        // Non-agent kinds never carry WC_PYTHON, even when resolved.
        let env = build_env(
            "coordinator",
            &EnvInputs {
                wc_python: FRAMEWORK_PYTHON,
                ..Default::default()
            },
        );
        assert!(!env.iter().any(|(k, _)| k == "WC_PYTHON"));
    }

    #[test]
    fn build_env_disk_cleanup_never_carries_hf_token() {
        let env = build_env(
            "disk-cleanup",
            &EnvInputs {
                hf_token: "hf_secret",
                google_cloud_project: Some("proj"),
                ..Default::default()
            },
        );
        assert_eq!(
            env,
            vec![
                ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
                ("GOOGLE_CLOUD_PROJECT".to_string(), "proj".to_string()),
            ]
        );
        assert!(!env.iter().any(|(k, _)| k.contains("HF")));
    }

    #[test]
    fn build_env_failure_fixer_forwards_path_and_bucket() {
        let env = build_env(
            "failure-fixer",
            &EnvInputs {
                hf_token: "hf_secret",
                path: Some("/opt/homebrew/bin:/usr/bin"),
                google_cloud_project: Some("proj"),
                gcp_project: None,
                wc_bucket: None,
                ..Default::default()
            },
        );
        assert_eq!(
            env,
            vec![
                ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
                ("HF_TOKEN".to_string(), "hf_secret".to_string()),
                (
                    "HUGGING_FACE_HUB_TOKEN".to_string(),
                    "hf_secret".to_string()
                ),
                ("PATH".to_string(), "/opt/homebrew/bin:/usr/bin".to_string()),
                ("GOOGLE_CLOUD_PROJECT".to_string(), "proj".to_string()),
                ("WC_BUCKET".to_string(), "wisent-compute".to_string()),
            ]
        );
    }

    #[test]
    fn renderings_match_goldens() {
        let env = vec![
            ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
            ("WC_PYTHON".to_string(), FRAMEWORK_PYTHON.to_string()),
            ("HF_TOKEN".to_string(), "hf_secret".to_string()),
            (
                "HUGGING_FACE_HUB_TOKEN".to_string(),
                "hf_secret".to_string(),
            ),
        ];
        let args = vec![
            "/Users/u/.stado/bin/stado".to_string(),
            "agent".to_string(),
            "--auto".to_string(),
        ];
        assert_eq!(
            plist_text("com.wisent.compute.agent.mini-one", &args, &env),
            include_str!("testdata/local_install_agent.plist")
        );
        assert_eq!(
            systemd_user_unit("Wisent Compute agent (mini-one)", &args, &env),
            include_str!("testdata/local_install_agent.service")
        );
    }

    #[test]
    fn dry_run_lines_match_python_format() {
        let plan = InstallPlan {
            name: "mini-one".to_string(),
            kind: "agent".to_string(),
            os: LocalOs::Darwin,
            label: label("agent", "mini-one"),
            exec_args: vec![
                "/Users/u/.stado/bin/stado".to_string(),
                "agent".to_string(),
                "--auto".to_string(),
            ],
            env: vec![
                ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
                ("WC_PYTHON".to_string(), FRAMEWORK_PYTHON.to_string()),
                ("HF_TOKEN".to_string(), "hf_secret".to_string()),
            ],
        };
        assert_eq!(
            plan.dry_run_lines(),
            vec![
                "[dry-run] agent=mini-one on Darwin".to_string(),
                "  exec: /Users/u/.stado/bin/stado agent --auto".to_string(),
                format!(
                    "  env:  {{'PYTHONUNBUFFERED': '1', 'WC_PYTHON': '{FRAMEWORK_PYTHON}', 'HF_TOKEN': 'hf_secret'}}"
                ),
            ]
        );
    }

    #[test]
    fn command_argv_matches_python() {
        let path =
            Path::new("/home/u/Library/LaunchAgents/com.wisent.compute.agent.mini-one.plist");
        let [bootout, bootstrap, kickstart] =
            darwin_commands("com.wisent.compute.agent.mini-one", path, 501);
        assert_eq!(
            bootout.argv,
            vec![
                "launchctl",
                "bootout",
                "gui/501/com.wisent.compute.agent.mini-one"
            ]
        );
        assert_eq!(
            bootstrap.argv,
            vec![
                "launchctl",
                "bootstrap",
                "gui/501",
                "/home/u/Library/LaunchAgents/com.wisent.compute.agent.mini-one.plist",
            ]
        );
        assert_eq!(
            kickstart.argv,
            vec![
                "launchctl",
                "kickstart",
                "-k",
                "gui/501/com.wisent.compute.agent.mini-one"
            ]
        );
        let [reload, enable] = linux_commands("com.wisent.compute.agent.mini-one");
        assert_eq!(reload.argv, vec!["systemctl", "--user", "daemon-reload"]);
        assert_eq!(
            enable.argv,
            vec![
                "systemctl",
                "--user",
                "enable",
                "--now",
                "com.wisent.compute.agent.mini-one.service"
            ]
        );
    }

    fn darwin_plan() -> InstallPlan {
        InstallPlan {
            name: "mini-one".to_string(),
            kind: "agent".to_string(),
            os: LocalOs::Darwin,
            label: label("agent", "mini-one"),
            exec_args: vec![
                "/Users/u/.stado/bin/stado".to_string(),
                "agent".to_string(),
                "--auto".to_string(),
            ],
            env: vec![("PYTHONUNBUFFERED".to_string(), "1".to_string())],
        }
    }

    #[tokio::test]
    async fn execute_darwin_retries_bootstrap_then_kickstarts() {
        let home = tempfile::tempdir().unwrap();
        let plan = darwin_plan();
        let (runner, calls) = fake_runner(vec![
            out(0, "", ""),
            out(1, "", "busy"),
            out(0, "", ""),
            out(0, "", ""),
        ]);
        let mut lines: Vec<String> = Vec::new();
        execute_plan(&plan, home.path(), 501, &runner, &mut |l| {
            lines.push(l.to_string())
        })
        .await
        .unwrap();
        let plist = plan.unit_path(home.path());
        assert_eq!(std::fs::read_to_string(&plist).unwrap(), plan.content());
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 4); // bootout, bootstrap(fail), bootstrap(ok), kickstart
        assert_eq!(calls[1], calls[2]);
        drop(calls);
        assert_eq!(
            lines,
            vec![
                format!("[plist] wrote {}", plist.display()),
                "[ok]   loaded launchd job com.wisent.compute.agent.mini-one (logs: /tmp/com.wisent.compute.agent.mini-one.log)".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn execute_darwin_is_idempotent_on_unchanged_content() {
        let home = tempfile::tempdir().unwrap();
        let plan = darwin_plan();
        let outputs = vec![out(0, "", ""), out(0, "", ""), out(0, "", "")];
        let (runner, _calls) = fake_runner(outputs.clone());
        let mut lines: Vec<String> = Vec::new();
        execute_plan(&plan, home.path(), 501, &runner, &mut |l| {
            lines.push(l.to_string())
        })
        .await
        .unwrap();
        let (runner2, _calls2) = fake_runner(outputs);
        let mut lines2: Vec<String> = Vec::new();
        execute_plan(&plan, home.path(), 501, &runner2, &mut |l| {
            lines2.push(l.to_string())
        })
        .await
        .unwrap();
        assert!(lines2[0].starts_with("[plist] unchanged "), "{lines2:?}");
    }

    #[tokio::test]
    async fn execute_darwin_bootstrap_failure_is_click_style_error() {
        let home = tempfile::tempdir().unwrap();
        let plan = darwin_plan();
        let mut outputs = vec![out(0, "", "")];
        outputs.extend(std::iter::repeat_n(out(37, "", "Boot-out failed"), 5));
        let (runner, calls) = fake_runner(outputs);
        let mut lines: Vec<String> = Vec::new();
        let err = execute_plan(&plan, home.path(), 501, &runner, &mut |l| {
            lines.push(l.to_string())
        })
        .await
        .unwrap_err();
        assert_eq!(err.0, "launchctl bootstrap failed: Boot-out failed");
        assert_eq!(calls.lock().unwrap().len(), 6); // bootout + 5 attempts
    }

    #[tokio::test]
    async fn execute_linux_enable_failure_is_click_style_error() {
        let home = tempfile::tempdir().unwrap();
        let plan = InstallPlan {
            os: LocalOs::Linux,
            ..darwin_plan()
        };
        let (runner, _calls) = fake_runner(vec![out(0, "", ""), out(1, "", "no bus")]);
        let mut lines: Vec<String> = Vec::new();
        let err = execute_plan(&plan, home.path(), 501, &runner, &mut |l| {
            lines.push(l.to_string())
        })
        .await
        .unwrap_err();
        assert_eq!(err.0, "systemctl enable failed: no bus");
        assert_eq!(
            lines[0],
            format!("[unit] wrote {}", plan.unit_path(home.path()).display())
        );
    }

    /// Offline release channel: `latest.json`, a SHA256SUMS over
    /// `binaries`, and every binary under `<version>/<platform>/`.
    struct ReleaseFixture {
        objects: std::collections::HashMap<String, Vec<u8>>,
        /// When set, every fetch is a transport failure instead of a
        /// lookup — the unreachable-channel case.
        error: Option<String>,
    }

    #[async_trait::async_trait]
    impl crate::self_update::ReleaseFetcher for ReleaseFixture {
        async fn fetch(
            &self,
            object_path: &str,
        ) -> Result<Option<Vec<u8>>, crate::self_update::SelfUpdateError> {
            if let Some(message) = &self.error {
                return Err(crate::self_update::SelfUpdateError::Fetch(message.clone()));
            }
            Ok(self.objects.get(object_path).cloned())
        }
    }

    /// `tamper` corrupts one object AFTER its checksum was computed.
    fn release_fixture(
        version: &str,
        binaries: &[(&str, &[u8])],
        tamper: Option<&str>,
    ) -> ReleaseFixture {
        use crate::self_update::{sha256_hex, LATEST_JSON_NAME, SHA256SUMS_NAME};
        let mut objects = std::collections::HashMap::new();
        objects.insert(
            LATEST_JSON_NAME.to_string(),
            format!(r#"{{"version": "{version}", "channel": "stable"}}"#).into_bytes(),
        );
        let sums = binaries
            .iter()
            .map(|(name, bytes)| format!("{}  {}", sha256_hex(bytes), name))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let prefix = format!("{version}/{}", platform_str());
        objects.insert(format!("{prefix}/{SHA256SUMS_NAME}"), sums.into_bytes());
        for (name, bytes) in binaries {
            let content = if Some(*name) == tamper {
                b"tampered".to_vec()
            } else {
                bytes.to_vec()
            };
            objects.insert(format!("{prefix}/{name}"), content);
        }
        ReleaseFixture {
            objects,
            error: None,
        }
    }

    #[tokio::test]
    async fn ensure_bins_downloads_verifies_and_is_idempotent() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let fetcher = release_fixture(
            "9.9.9",
            &[
                ("stado", b"new-stado"),
                ("stado-fix", b"new-fix"),
                ("stado-watchdog", b"new-wd"),
            ],
            None,
        );
        let mut lines: Vec<String> = Vec::new();
        ensure_bins_with(home.path(), &fetcher, &mut |l| lines.push(l.to_string()))
            .await
            .unwrap();
        let bin_dir = home.path().join(".stado").join("bin");
        assert_eq!(std::fs::read(bin_dir.join("stado")).unwrap(), b"new-stado");
        assert_eq!(
            std::fs::read(bin_dir.join("stado-fix")).unwrap(),
            b"new-fix"
        );
        assert_eq!(
            std::fs::read(bin_dir.join("stado-watchdog")).unwrap(),
            b"new-wd"
        );
        assert_eq!(
            std::fs::metadata(bin_dir.join("stado"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            lines,
            vec![format!(
                "[install] downloaded stado 9.9.9 ({}) -> {}",
                platform_str(),
                bin_dir.display()
            )]
        );
        // Fully populated: an empty channel proves no-op — any fetch
        // would miss and fail the install.
        let fetcher = ReleaseFixture {
            objects: std::collections::HashMap::new(),
            error: None,
        };
        let mut lines: Vec<String> = Vec::new();
        ensure_bins_with(home.path(), &fetcher, &mut |l| lines.push(l.to_string()))
            .await
            .unwrap();
        assert!(lines.is_empty());
    }

    fn platform_str() -> &'static str {
        release_platform().unwrap()
    }

    #[tokio::test]
    async fn ensure_bins_hash_mismatch_installs_nothing() {
        let home = tempfile::tempdir().unwrap();
        let fetcher = release_fixture(
            "9.9.9",
            &[
                ("stado", b"new-stado"),
                ("stado-fix", b"new-fix"),
                ("stado-watchdog", b"new-wd"),
            ],
            Some("stado-fix"),
        );
        let mut lines: Vec<String> = Vec::new();
        let err = ensure_bins_with(home.path(), &fetcher, &mut |l| lines.push(l.to_string()))
            .await
            .unwrap_err();
        assert!(err.0.starts_with("sha256 mismatch for stado-fix"), "{err}");
        assert!(!home
            .path()
            .join(".stado")
            .join("bin")
            .join("stado")
            .exists());
    }

    #[tokio::test]
    async fn ensure_bins_release_lookup_failure_is_error() {
        let home = tempfile::tempdir().unwrap();
        let fetcher = ReleaseFixture {
            objects: std::collections::HashMap::new(),
            error: Some("channel unreachable".to_string()),
        };
        let mut lines: Vec<String> = Vec::new();
        let err = ensure_bins_with(home.path(), &fetcher, &mut |l| lines.push(l.to_string()))
            .await
            .unwrap_err();
        assert!(err.0.starts_with("release lookup failed:"), "{err}");
        assert!(err.0.contains("channel unreachable"), "{err}");
    }
}
