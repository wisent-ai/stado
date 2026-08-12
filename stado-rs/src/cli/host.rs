//! `stado host ...` — Rust implementations of the complete `host` group:
//! health, recovery, user provisioning, and Weles recordings policy, plus
//! the read-only diagnostics of `docs/missing-commands.md` items two
//! through six (`uptime`, `ping`, `disk`, `cleanup --dry-run`, `exec`),
//! which have no Python original and live in `crate::deploy::host_*`.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::io::Read;
use std::os::unix::fs::MetadataExt;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::CmdError;
use crate::deploy::host_users::{provision_users, ProvisionOptions};
use crate::targets::{ComputeTarget, Registry};

/// The store the health beacons live in.
///
/// GCS retains its historical registry bucket. Provider-neutral backends
/// keep registry and beacon objects in the configured JobStorage, so a GCS
/// locator must never be reinterpreted as an Azure container or S3 bucket.
pub(crate) async fn beacon_store() -> Result<crate::queue::JobStorage, CmdError> {
    let gcs_backend = crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
        == Some(crate::capabilities::StorageAdapter::Gcs);
    if !gcs_backend {
        return Ok(crate::queue::JobStorage::new().await?);
    }
    let bucket = crate::targets::GCS_REGISTRY_URI
        .split_once("//")
        .map(|(_, rest)| rest.split('/').next().unwrap_or_default())
        .unwrap_or_default();
    Ok(crate::queue::JobStorage::with_bucket(bucket).await?)
}

/// `stado host health TARGET [--json]` — show the latest Stado health
/// beacon and log tail for TARGET (Python `host_health` in cli.py: a
/// click.ClickException on FileNotFoundError/OSError/ValueError).
pub async fn health(target: &str, json: bool) -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let report = crate::monitor::host_health::load_host_health(&store, target)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report.to_json())?);
    } else {
        println!(
            "{}",
            crate::monitor::host_health::format_host_health(&report)
        );
    }
    Ok(())
}
/// `stado host publish-beacon FILE` — publish a locally collected health
/// document through the dedicated, route-scoped Stado control API.
///
/// This command deliberately has no direct-storage mode and does not consult
/// provider credentials. Missing URL/token configuration, an insecure remote
/// URL, an over-broad token file, malformed JSON, and an inconsistent server
/// acknowledgement all fail closed.
pub async fn publish_beacon(source: &str) -> Result<(), CmdError> {
    let bytes = if source == "-" {
        let mut bytes = Vec::new();
        std::io::stdin().lock().read_to_end(&mut bytes)?;
        bytes
    } else {
        std::fs::read(source)?
    };
    if bytes.is_empty() || bytes.len() > usize::from(u16::MAX) {
        return Err(CmdError::click(
            "host beacon must contain between one and 65535 bytes",
        ));
    }
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|error| CmdError::click(format!("host beacon is not valid JSON: {error}")))?;
    let host = document
        .as_object()
        .and_then(|value| value.get("host"))
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("host beacon must be an object with a string host"))?;
    if !valid_beacon_host(host) {
        return Err(CmdError::click(
            "host beacon host must be a lowercase DNS label",
        ));
    }
    if document
        .get("reported_at")
        .and_then(Value::as_str)
        .is_none()
        || document.get("units").and_then(Value::as_object).is_none()
    {
        return Err(CmdError::click(
            "host beacon requires string reported_at and object units fields",
        ));
    }

    let mut endpoint = host_health_api_url()?;
    {
        let mut segments = endpoint.path_segments_mut().map_err(|()| {
            CmdError::click("STADO_HOST_HEALTH_API_URL cannot be used as an HTTP API base URL")
        })?;
        segments.pop_if_empty();
        segments.push("api");
        segments.push("host-health");
    }
    endpoint.query_pairs_mut().append_pair("host", host);

    let token = host_health_api_token().await?;
    let response = crate::cli::storage::fleet_https_client()
        .map_err(|error| CmdError::click(error.to_string()))?
        .put(endpoint)
        .bearer_auth(&token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(bytes)
        .send()
        .await?;
    let status = response.status();
    let response_bytes = response.bytes().await?;
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&response_bytes).replace(&token, "[REDACTED]");
        return Err(CmdError::click(format!(
            "Stado host-health API returned HTTP {status}: {}",
            detail.trim()
        )));
    }
    let payload: Value = serde_json::from_slice(&response_bytes).map_err(|error| {
        CmdError::click(format!(
            "Stado host-health API returned invalid JSON: {error}"
        ))
    })?;
    // The publisher checks that the server stored THIS host's beacon, and
    // nothing about where. Reconstructing the server's storage layout here
    // made a correct publication fail on any host whose namespace differs
    // from the control plane's -- the client was asserting an internal detail
    // it has no way to know.
    let stored = payload.get("state").and_then(Value::as_str) == Some("stored")
        && payload.get("host").and_then(Value::as_str) == Some(&host[..])
        && payload
            .get("path")
            .and_then(Value::as_str)
            .is_some_and(|path| path.ends_with(&format!("{host}.json")));
    if !stored {
        return Err(CmdError::click(
            "Stado host-health API returned an inconsistent publish response",
        ));
    }
    println!("{host}");
    Ok(())
}

fn valid_beacon_host(host: &str) -> bool {
    let bytes = host.as_bytes();
    !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

fn host_health_api_url() -> Result<url::Url, CmdError> {
    let raw = std::env::var("STADO_HOST_HEALTH_API_URL")
        .map_err(|_| CmdError::click("STADO_HOST_HEALTH_API_URL is required"))?;
    let url = url::Url::parse(raw.trim())
        .map_err(|error| CmdError::click(format!("invalid STADO_HOST_HEALTH_API_URL: {error}")))?;
    let host = url
        .host_str()
        .ok_or_else(|| CmdError::click("STADO_HOST_HEALTH_API_URL must be an absolute URL"))?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(CmdError::click(
            "STADO_HOST_HEALTH_API_URL must use HTTPS unless its host is loopback",
        ));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CmdError::click(
            "STADO_HOST_HEALTH_API_URL must not contain credentials, query, or fragment",
        ));
    }
    Ok(url)
}

/// The publisher's bearer from an owner-only file, for a host that cannot
/// reach Skarbiec.
///
/// Skarbiec binds to loopback and the tailnet ingress carries only the object
/// API, so a Linux registry host has no authenticated path to a broker and
/// published no beacon at all -- `host ping` called a machine that was serving
/// releases "down". Every other grant in this fleet already lives as an
/// owner-only file; this reads the same shape. The bare value in the
/// environment stays forbidden, which is what `host recover` refuses as an
/// ambient credential.
fn host_health_api_token_from_file() -> Result<Option<String>, CmdError> {
    let Ok(raw) = std::env::var("STADO_HOST_HEALTH_API_TOKEN_FILE") else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let path = crate::config_file::expand_tilde(raw.trim());
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        CmdError::click(format!(
            "cannot inspect STADO_HOST_HEALTH_API_TOKEN_FILE {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(CmdError::click(format!(
            "STADO_HOST_HEALTH_API_TOKEN_FILE must be a regular file: {}",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(CmdError::click(format!(
                "STADO_HOST_HEALTH_API_TOKEN_FILE must be owner-only (chmod 600): {}",
                path.display()
            )));
        }
    }
    let token = std::fs::read_to_string(&path)
        .map_err(|error| {
            CmdError::click(format!(
                "cannot read STADO_HOST_HEALTH_API_TOKEN_FILE {}: {error}",
                path.display()
            ))
        })?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(CmdError::click(
            "STADO_HOST_HEALTH_API_TOKEN_FILE is empty",
        ));
    }
    Ok(Some(token))
}

async fn host_health_api_token() -> Result<String, CmdError> {
    if let Some(token) = host_health_api_token_from_file()? {
        return Ok(token);
    }
    let url = std::env::var("STADO_HOST_HEALTH_SKARBIEC_URL")
        .map_err(|_| CmdError::click("STADO_HOST_HEALTH_SKARBIEC_URL is required"))?;
    let consumer = std::env::var("STADO_HOST_HEALTH_SKARBIEC_CONSUMER")
        .map_err(|_| CmdError::click("STADO_HOST_HEALTH_SKARBIEC_CONSUMER is required"))?;
    if consumer != "stado-host-health-beacon" {
        return Err(CmdError::click(
            "STADO_HOST_HEALTH_SKARBIEC_CONSUMER must be stado-host-health-beacon",
        ));
    }
    let raw = std::env::var("STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE")
        .map_err(|_| CmdError::click("STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE is required"))?;
    let token_file = crate::config_file::expand_tilde(raw.trim())
        .to_string_lossy()
        .into_owned();
    let client = crate::skarbiec::Client::new(url.trim(), &consumer, &token_file)
        .map_err(|error| CmdError::click(error.to_string()))?;
    // One field, named. The whole-item read this used to do is exactly what
    // the broker stopped answering, and the beacon died with it: the host
    // published nothing for twenty-one hours while `stado service list` went
    // on reporting its stale `active` for services that were not running.
    let token = client
        .read_string("stado-host-health-api", "token")
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .unwrap_or_default()
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(CmdError::click(
            "Skarbiec item stado-host-health-api field token is required",
        ));
    }
    Ok(token)
}

/// `stado host recover TARGET` — recover a registry-managed macOS host
/// through its approved channel (Python `host_recover` in cli.py: prints
/// the report as sorted-keys JSON, exits 1 when status != "ok").
pub async fn recover(target: &str) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_recovery::recover_host(target, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    println!(
        "{}",
        crate::deploy::host_recovery::to_sorted_pretty(&report)
    );
    if report.get("status").and_then(Value::as_str) != Some("ok") {
        // click.exceptions.Exit(1): nothing more to print.
        return Err(CmdError::silent(1));
    }
    Ok(())
}

/// `stado host reboot TARGET` — request a graceful reboot through the
/// approved channel (`docs/missing-commands.md` item one).
///
/// [`crate::deploy::host_reboot`] has been complete since July but was
/// never reachable: `deploy/mod.rs` did not declare the module and no CLI
/// variant dispatched to it, so the command the incident write-up records
/// as shipped did not exist. Both halves are wired now.
pub async fn reboot(target: &str) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_reboot::reboot_host(target, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    print_json(&report);
    report_outcome(&report, "reboot_requested")
}

/// Resolve TARGET in the canonical registry, the same source
/// `host weles-recordings-dir` writes back to.
async fn registry_target(target: &str) -> Result<ComputeTarget, CmdError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    registry
        .targets
        .iter()
        .find(|candidate| candidate.name == target)
        .cloned()
        .ok_or_else(|| CmdError::click(format!("unknown registry target: {target}")))
}

/// `stado host user delete USERNAME --target T [--keep-home]` — remove the
/// account through the channel that created it.
pub async fn user_delete(username: &str, target: &str, keep_home: bool) -> Result<(), CmdError> {
    let resolved = registry_target(target).await?;
    let runner = crate::deploy::production_runner();
    let result =
        crate::deploy::host_user_delete::delete_user(username, &resolved, keep_home, &runner).await;
    match result.error {
        Some(detail) => Err(CmdError::click(detail)),
        None => {
            println!("{}\t{}\t{}", result.target, result.status, username);
            Ok(())
        }
    }
}

/// `stado host build-caches report|prune TARGET --root PATH --min-age-days N`
/// — the disk cleaner covers model caches and recordings, not build output,
/// which is what actually fills a developer host.
pub async fn build_caches(
    target: &str,
    root: &str,
    min_age_days: &str,
    apply: bool,
) -> Result<(), CmdError> {
    let resolved = registry_target(target).await?;
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_build_caches::run_on_host(
        &resolved,
        root,
        min_age_days,
        apply,
        &runner,
    )
    .await;
    let mut total_kib: u64 = u64::default();
    for entry in &report.entries {
        println!(
            "{}\t{}\t{}\t{}",
            report.target, entry.state, entry.kib, entry.path
        );
        total_kib += entry.kib.parse::<u64>().unwrap_or_default();
    }
    println!("{}\ttotal-kib\t{total_kib}", report.target);
    match report.error {
        Some(detail) if !detail.is_empty() => Err(CmdError::click(detail)),
        Some(_) => Err(CmdError::click("remote command failed".to_string())),
        None => Ok(()),
    }
}
fn print_report(
    report: &crate::deploy::host_gui_automation::GuiAutomationReport,
) -> Result<(), CmdError> {
    for (item, state) in &report.items {
        println!("{}\t{item}\t{state}", report.target);
    }
    match &report.error {
        Some(detail) if !detail.is_empty() => Err(CmdError::click(detail.clone())),
        Some(_) => Err(CmdError::click("remote command failed".to_string())),
        None => Ok(()),
    }
}

/// `stado host gui-automation status TARGET` — report autologin, remote
/// management, VNC, automation artifacts and the console owner.
pub async fn gui_automation_status(target: &str) -> Result<(), CmdError> {
    let resolved = registry_target(target).await?;
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_gui_automation::run_on_host(
        &resolved,
        crate::deploy::host_gui_automation::REMOTE_STATUS_SCRIPT,
        "",
        &runner,
    )
    .await;
    print_report(&report)
}

/// `stado host gui-automation disable TARGET [--bundle ID]` — revert the
/// enablement and report every item it touched.
pub async fn gui_automation_disable(target: &str, bundle: &str) -> Result<(), CmdError> {
    let resolved = registry_target(target).await?;
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_gui_automation::run_on_host(
        &resolved,
        crate::deploy::host_gui_automation::REMOTE_DISABLE_SCRIPT,
        bundle,
        &runner,
    )
    .await;
    print_report(&report)
}

/// Read one line with terminal echo disabled (Python
/// `click.prompt(..., hide_input=True)`).
fn prompt_hidden(prompt: &str) -> Result<String, CmdError> {
    use std::io::Write;
    print!("{prompt}: ");
    std::io::stdout().flush()?;
    // SAFETY: isatty/tcgetattr/tcsetattr on fd 0 with a valid termios
    // buffer; the original settings are always restored below.
    let tty = unsafe { nix::libc::isatty(nix::libc::STDIN_FILENO) } == 1;
    let mut original: nix::libc::termios = unsafe { std::mem::zeroed() };
    if tty {
        unsafe {
            nix::libc::tcgetattr(nix::libc::STDIN_FILENO, &mut original);
            let mut hidden = original;
            hidden.c_lflag &= !nix::libc::ECHO;
            nix::libc::tcsetattr(nix::libc::STDIN_FILENO, nix::libc::TCSANOW, &hidden);
        }
    }
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line);
    if tty {
        // SAFETY: restores the settings captured above.
        unsafe {
            nix::libc::tcsetattr(nix::libc::STDIN_FILENO, nix::libc::TCSANOW, &original);
        }
        println!();
    }
    read?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Python `click.prompt("Initial password", hide_input=True,
/// confirmation_prompt=True)`: re-prompts until the two entries match.
fn prompt_initial_password() -> Result<String, CmdError> {
    loop {
        let first = prompt_hidden("Initial password")?;
        let second = prompt_hidden("Repeat for confirmation")?;
        if first == second {
            return Ok(first);
        }
        eprintln!("Error: The two entered values do not match.");
    }
}

/// The registry for `--registry-source` (Python `load_targets(source=...)`:
/// "gcs" = the canonical remote registry only (whichever store
/// `WC_STORAGE_BACKEND` selects), "local" = bundled file, "auto" = remote
/// with bundled fallback).
async fn load_registry_by_source(source: &str) -> Result<Registry, CmdError> {
    match source {
        "gcs" => crate::targets::fetch_registry_remote()
            .await
            .map_err(|exc| CmdError::click(exc.to_string())),
        "local" => {
            crate::targets::load_bundled_registry().map_err(|exc| CmdError::click(exc.to_string()))
        }
        _ => crate::targets::load_registry_auto()
            .await
            .map_err(|exc| CmdError::click(exc.to_string())),
    }
}

/// `stado host user create USERNAME ...` — create the account on selected
/// registry-managed hosts over SSH (Python `host_user_create` in cli.py).
#[allow(clippy::too_many_arguments)]
pub async fn user_create(
    username: &str,
    target_names: Vec<String>,
    all_targets: bool,
    full_name: Option<String>,
    shell: String,
    admin: bool,
    require_password_change: bool,
    dry_run: bool,
    registry_source: &str,
) -> Result<(), CmdError> {
    let password = if dry_run {
        None
    } else {
        Some(prompt_initial_password()?)
    };
    let registry = load_registry_by_source(registry_source).await?;
    let targets: Vec<&ComputeTarget> = registry.targets.iter().collect();
    let runner = crate::deploy::production_runner();
    let options = ProvisionOptions {
        username,
        password: password.as_deref(),
        target_names: &target_names,
        all_targets,
        full_name: full_name.as_deref(),
        shell: &shell,
        admin,
        require_password_change,
        dry_run,
    };
    let results = provision_users(&options, &targets, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    for result in &results {
        match result.status.as_str() {
            "failed" => {
                eprintln!(
                    "[failed] {} ({}): {}",
                    result.target, result.ssh, result.detail
                );
            }
            "planned" => {
                println!(
                    "[plan]   {}: create {} via {}",
                    result.target, username, result.ssh
                );
            }
            status => {
                println!(
                    "[{status}] {}: {} on {} via {}",
                    result.target, username, result.os_name, result.ssh
                );
            }
        }
    }
    if results.iter().any(|result| !result.ok()) {
        // click.exceptions.Exit(1): the [failed] lines above are the report.
        return Err(CmdError::silent(1));
    }
    Ok(())
}

/// `stado host weles-recordings-dir TARGET PATH` — update the canonical
/// registry with generation fencing, then update local Weles LaunchAgents
/// when TARGET resolves to this host.
///
/// The registry object is resolved by [`crate::targets::RegistryStore`],
/// the same seam `cli/registry.rs::push` writes through, so this repairs
/// the registry on whichever store `WC_STORAGE_BACKEND` selects. It used
/// to build a `GcsBackend` on a hardcoded bucket and so failed closed on
/// an Azure-only deployment.
pub async fn weles_recordings_dir(target: &str, path: &str) -> Result<(), CmdError> {
    use serde_json::{json, Map};

    if !std::path::Path::new(path).is_absolute() {
        return Err(CmdError::click("PATH must be absolute"));
    }

    let store = crate::targets::RegistryStore::open().await?;
    let current = store
        .read_versioned()
        .await?
        .ok_or_else(|| CmdError::click("canonical registry generation unavailable"))?;
    let mut document: Value = serde_json::from_str(&current.content)?;
    let targets = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?;
    let entry = targets
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(target))
        .ok_or_else(|| CmdError::click(format!("target not in registry: {target}")))?;
    let entry = entry
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry target must be an object"))?;

    let weles = entry
        .entry("weles")
        .or_insert_with(|| json!({"enabled": false, "actions": []}))
        .as_object_mut()
        .ok_or_else(|| CmdError::click("weles must be an object"))?;
    weles.insert(
        "recordings_dir".to_string(),
        Value::String(path.to_string()),
    );

    if let Some(cleanup) = entry.get_mut("disk_cleanup").and_then(Value::as_object_mut) {
        let cleaners = cleanup
            .entry("cleaners")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| CmdError::click("disk_cleanup.cleaners must be an object"))?;
        let cleaner = cleaners
            .entry("weles_recordings")
            .or_insert_with(|| {
                let min_age = "604800".parse::<i64>().expect("constant integer");
                json!({"min_age_seconds": min_age})
            })
            .as_object_mut()
            .ok_or_else(|| CmdError::click("weles_recordings cleaner must be an object"))?;
        cleaner.insert("root".to_string(), Value::String(path.to_string()));
    }

    crate::targets::validate_registry(&document).map_err(|exc| CmdError::click(exc.to_string()))?;
    let payload = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let generation = store.compare_and_swap(&current.version, &payload).await?;
    println!("registry: {target} weles.recordings_dir={path} (generation {generation})");

    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();
    let registry = crate::targets::load_registry_from_str(&payload)
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let is_self = registry
        .lookup_self(&hostname)
        .map_err(|exc| CmdError::click(exc.to_string()))?
        .is_some_and(|entry| entry.name == target);
    if !is_self {
        println!(
            "run `wc host weles-recordings-dir {target} {path}` on {target} to update its LaunchAgents"
        );
        return Ok(());
    }

    std::fs::create_dir_all(path)?;
    let agents_dir = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| CmdError::click("HOME is not set"))?
        .join("Library/LaunchAgents");
    let mut touched = usize::default();
    for item in std::fs::read_dir(&agents_dir)? {
        let plist = item?.path();
        let Some(name) = plist.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("com.wisent.weles-") || !name.ends_with(".plist") {
            continue;
        }
        set_plist_recordings_root(&plist, path)?;
        touched += usize::from(true);
        println!("  {name}: WELES_RECORDINGS_ROOT={path}");
    }
    println!("updated {touched} LaunchAgent plist(s); reload weles agents to apply");
    Ok(())
}

fn set_plist_recordings_root(plist: &std::path::Path, path: &str) -> Result<(), CmdError> {
    fn plutil(plist: &std::path::Path, args: &[&str]) -> std::io::Result<std::process::Output> {
        std::process::Command::new("/usr/bin/plutil")
            .args(args)
            .arg(plist)
            .output()
    }

    let key = "EnvironmentVariables.WELES_RECORDINGS_ROOT";
    let replace = plutil(plist, &["-replace", key, "-string", path])?;
    if replace.status.success() {
        return Ok(());
    }
    let insert = plutil(plist, &["-insert", key, "-string", path])?;
    if insert.status.success() {
        return Ok(());
    }
    let _ = plutil(
        plist,
        &["-insert", "EnvironmentVariables", "-xml", "<dict/>"],
    )?;
    let retry = plutil(plist, &["-insert", key, "-string", path])?;
    if retry.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&retry.stderr).trim().to_string();
    Err(CmdError::click(format!(
        "{}: failed to update WELES_RECORDINGS_ROOT: {message}",
        plist.display()
    )))
}

// ---------------------------------------------------------------------------
// docs/missing-commands.md items two through six
//
// Each of these is a thin shell over one `crate::deploy` module: resolve,
// run through the shared ssh channel, then either print the report as JSON
// or render it. NO Python original — the Python CLI stops at
// `host recover`.
//
// Two conventions hold across all five. `--json` prints the deploy
// module's report with sorted keys, exactly as `host recover` prints its
// own (`deploy::host_recovery::to_sorted_pretty`). A non-zero remote exit
// is a click error carrying the remote's own last line, so the shell exit
// status of `stado host ...` matches the health of the host.
// ---------------------------------------------------------------------------

/// Print `report` as sorted-keys JSON, the way `host recover` prints its
/// own report.
fn print_json(report: &Value) {
    println!("{}", crate::deploy::host_recovery::to_sorted_pretty(report));
}

/// A report's `error` field, or its `status`, as a click error.
///
/// Returns `Ok` when the report is healthy. `expected` is the `status`
/// value that means "this command did what it was asked to do"; anything
/// else is a failure the exit status has to reflect.
fn report_outcome(report: &Value, expected: &str) -> Result<(), CmdError> {
    let status = report.get("status").and_then(Value::as_str).unwrap_or("");
    if status == expected {
        return Ok(());
    }
    let detail = report.get("error").and_then(Value::as_str).map_or_else(
        || format!("host reported status {status:?}"),
        str::to_string,
    );
    Err(CmdError::click(detail))
}

/// A JSON value as one table cell: strings bare, null as a dash, anything
/// else in its JSON spelling.
fn cell(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "-".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// `stado host uptime TARGET [--json]` — uptime, load averages and
/// logged-in users (`docs/missing-commands.md` item two).
pub async fn uptime(target: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_uptime::uptime_host(target, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    if json {
        print_json(&report);
        return report_outcome(&report, crate::deploy::host_uptime::OK_STATUS);
    }
    let load = report.get("load_average");
    let field = |key: &str| cell(load.and_then(|value| value.get(key)));
    println!("host:    {}", cell(report.get("host")));
    println!("uptime:  {}", cell(report.get("uptime")));
    println!(
        "load:    {} (1m)  {} (5m)  {} (15m)",
        field("one"),
        field("five"),
        field("fifteen")
    );
    let users = report
        .get("users")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if users.is_empty() {
        println!("users:   none logged in");
    } else {
        let rows: Vec<Vec<String>> = users
            .iter()
            .map(|user| {
                vec![
                    cell(user.get("user")),
                    cell(user.get("line")),
                    cell(user.get("since")),
                ]
            })
            .collect();
        super::table::print(&["USER", "LINE", "SINCE"], &rows);
    }
    report_outcome(&report, crate::deploy::host_uptime::OK_STATUS)
}

/// `stado host ping TARGET [--json]` — ssh reachability and beacon age in
/// one verdict (`docs/missing-commands.md` item three).
///
/// The exit status follows the COMBINED verdict, so a box answering ssh
/// with a five-day-old beacon fails this command. That is the whole point
/// of it: the July incident's host would have passed an ssh-only ping
/// every one of those five days.
pub async fn ping(target: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let store = beacon_store().await?;
    let report = crate::deploy::host_ping::ping_host(target, &store, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let verdict = crate::deploy::host_ping::Verdict::Ok.as_str();
    if json {
        print_json(&report);
        return report_outcome(&report, verdict);
    }
    let ssh = report.get("ssh_check");
    let beacon = report.get("beacon");
    let part = |section: Option<&Value>, key: &str| cell(section.and_then(|v| v.get(key)));
    let rows = vec![
        vec![
            "ssh".to_string(),
            part(ssh, "status"),
            format!("answered as {}", part(ssh, "host")),
        ],
        vec![
            "beacon".to_string(),
            part(beacon, "status"),
            match beacon.and_then(|value| value.get("error")) {
                Some(Value::String(detail)) => detail.clone(),
                _ => format!(
                    "{} old, reported {}",
                    beacon_age(beacon),
                    part(beacon, "reported_at")
                ),
            },
        ],
    ];
    super::table::print(&["SIGNAL", "STATE", "DETAIL"], &rows);
    println!(
        "\nverdict: {} (the worse of the two signals)",
        cell(report.get("status"))
    );
    report_outcome(&report, verdict)
}

/// The beacon's age, in the spelling `stado registry beacon-age` already
/// uses for the same signal across the whole fleet.
fn beacon_age(section: Option<&Value>) -> String {
    section
        .and_then(|value| value.get("age_seconds"))
        .and_then(Value::as_i64)
        .map_or_else(
            || "-".to_string(),
            |age| super::registry::human_age(chrono::TimeDelta::seconds(age)),
        )
}

/// `stado host disk TARGET [--json]` — disk usage plus the registry
/// cleanup policy and its recorded state (`docs/missing-commands.md`
/// item four).
pub async fn disk(target: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_disk::disk_host(target, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let expected = crate::deploy::host_disk::OK_STATUS;
    if json {
        print_json(&report);
        return report_outcome(&report, expected);
    }
    let usage = report.get("usage");
    let used = |key: &str| cell(usage.and_then(|value| value.get(key)));
    super::table::print(
        &[
            "FILESYSTEM",
            "MOUNT",
            "BLOCKS KB",
            "USED KB",
            "AVAIL KB",
            "CAPACITY",
        ],
        &[vec![
            used("filesystem"),
            used("mounted_on"),
            used("blocks_kb"),
            used("used_kb"),
            used("available_kb"),
            used("capacity"),
        ]],
    );

    let policy = report.get("policy");
    let declared = |key: &str| cell(policy.and_then(|value| value.get(key)));
    if policy.is_none() || policy == Some(&Value::Null) {
        println!("\ncleanup policy: none declared in the registry for this target");
    } else {
        println!(
            "\ncleanup policy: mode={} every {}s, low={}GiB target={}GiB",
            declared("mode"),
            declared("check_interval_seconds"),
            declared("low_free_gb"),
            declared("target_free_gb"),
        );
    }

    let state = report.get("cleanup_state");
    let recorded = |key: &str| cell(state.and_then(|value| value.get(key)));
    if state.and_then(|value| value.get("present")) == Some(&Value::Bool(true)) {
        super::table::print(
            &[
                "LAST PASS",
                "OUTCOME",
                "FREED BYTES",
                "LAST SUCCESS",
                "NEXT PASS",
            ],
            &[vec![
                recorded("last_pass_at"),
                recorded("outcome"),
                recorded("freed_bytes"),
                recorded("last_success_at"),
                recorded("next_pass_at"),
            ]],
        );
        if let Some(Value::String(detail)) = state.and_then(|value| value.get("error")) {
            println!("cleanup state unreadable: {detail}");
        }
    } else {
        println!(
            "\ncleanup state: no state file at {} — the janitor has never \
             completed a pass on this host",
            recorded("path")
        );
    }
    report_outcome(&report, expected)
}

/// `stado host cleanup TARGET --dry-run [--json]` — preview what the
/// registry cleanup would delete (`docs/missing-commands.md` item five).
///
/// `--dry-run` is mandatory, not defaulted. This command only ever
/// previews: the enforcing pass belongs to the host's own janitor on the
/// interval its registry policy declares, and to `stado host recover`,
/// which runs it as part of a deliberate recovery. A flag that could be
/// omitted would eventually be omitted.
pub async fn cleanup(target: &str, dry_run: bool, json: bool) -> Result<(), CmdError> {
    if !dry_run {
        return Err(CmdError::usage(
            "host cleanup only previews; pass --dry-run. To actually reclaim space, let the \
             host's janitor run on its registry interval, or run stado host recover TARGET",
        ));
    }
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_cleanup::cleanup_preview(target, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let expected = crate::deploy::host_cleanup::PREVIEW_STATUS;
    if json {
        print_json(&report);
        return report_outcome(&report, expected);
    }
    println!("DRY RUN — nothing on {target} is deleted.");
    println!(
        "registry policy mode: {}",
        cell(report.get("registry_policy_mode"))
    );
    let plan = report.get("plan").filter(|value| !value.is_null());
    let Some(plan) = plan else {
        println!(
            "no plan: {}",
            cell(report.get("unavailable").or_else(|| report.get("error")))
        );
        return report_outcome(&report, expected);
    };
    println!("outcome:              {}", cell(plan.get("outcome")));
    println!(
        "free bytes:           {} (low watermark {})",
        cell(plan.get("free_bytes_before")),
        cell(plan.get("low_bytes"))
    );
    let rows: Vec<Vec<String>> = crate::deploy::host_cleanup::cleaner_plans(plan)
        .iter()
        .map(|cleaner| {
            vec![
                cleaner.name.clone(),
                cleaner.scanned_items.to_string(),
                cleaner.eligible_items.to_string(),
                cleaner.expected_bytes.to_string(),
                cleaner.deleted_items.to_string(),
            ]
        })
        .collect();
    super::table::print(
        &[
            "CLEANER",
            "SCANNED",
            "WOULD DELETE",
            "WOULD FREE BYTES",
            "DELETED",
        ],
        &rows,
    );
    println!("\nDELETED is zero by construction — this pass ran in the janitor's report mode.");
    report_outcome(&report, expected)
}

/// `stado host exec TARGET [--json] -- CMD…` — run one approved read-only
/// command (`docs/missing-commands.md` item six).
pub async fn exec(target: &str, words: Vec<String>, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_exec::exec_host(target, &words, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let expected = crate::deploy::host_exec::OK_STATUS;
    if json {
        print_json(&report);
        return report_outcome(&report, expected);
    }
    // The point of the command is the host's own output, so it is passed
    // through untouched rather than folded into a table.
    print!("{}", cell(report.get("stdout")));
    let stderr = cell(report.get("stderr"));
    if !stderr.is_empty() && stderr != "-" {
        eprint!("{stderr}");
    }
    report_outcome(&report, expected)
}

/// `stado host inventory TARGET [--json]` — the stado-managed binaries,
/// forward markers and loopback listeners of TARGET, and the verdict on
/// whether each marker still matches a live listener.
///
/// The only thing it takes is the registry target name. There is no path,
/// file name, port or pattern to pass, because a command that took one
/// would be a command that could be pointed at `~/.ssh/id_ed25519`.
/// `stado host vaults [TARGET]` — which Skarbiec vaults the fleet holds.
///
/// Without a target this asks every registry host, because "how many vaults
/// does this fleet have" is the question a machine cannot answer about
/// itself: a vault is a file, and the desktop client that lists them is
/// honest that it only sees the machine it runs on.
///
/// Only an owner, three counts and a path cross the wire. Skarbiec's own
/// documentation calls item, consumer and scope names "the map" and holds
/// them above the encrypted values in confidentiality, so a fleet sweep
/// reports how much is held and never what.
pub async fn vaults(target: Option<String>, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let names: Vec<String> = match target {
        Some(name) => vec![name],
        None => {
            let registry = crate::targets::fetch_registry_remote()
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
            registry.targets.iter().map(|entry| entry.name.clone()).collect()
        }
    };
    let mut hosts: Vec<serde_json::Value> = Vec::new();
    for name in &names {
        let resolved = crate::deploy::host_channel::canonical_target(name)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let answer = crate::deploy::fleet_vaults::collect_from(&resolved, &runner).await;
        hosts.push(crate::deploy::fleet_vaults::attribute(name, answer));
    }
    let summary = crate::deploy::fleet_vaults::summarize(&hosts);
    if json {
        print_json(&json!({"summary": summary, "hosts": hosts}));
        return Ok(());
    }
    for host in &hosts {
        let name = host.get("target").and_then(Value::as_str).unwrap_or("?");
        if let Some(error) = host.get("error").and_then(Value::as_str) {
            println!("{name}: {error}");
            continue;
        }
        if let Some(absent) = host.get("absent").and_then(Value::as_str) {
            println!("{name}: {absent}");
            continue;
        }
        let list = host.get("vaults").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        println!("{name}: {} vault(s)", list.len());
        for vault in list {
            println!(
                "  {:>5} items  {} recipients  {}",
                vault.get("items").and_then(Value::as_u64).unwrap_or_default(),
                vault.get("recipients").and_then(Value::as_u64).unwrap_or_default(),
                vault.get("path").and_then(Value::as_str).unwrap_or("")
            );
        }
    }
    println!(
        "{} host(s), {} unreachable, {} vault(s), {} item(s)",
        summary.get("hosts").and_then(Value::as_u64).unwrap_or_default(),
        summary.get("unreachable").and_then(Value::as_u64).unwrap_or_default(),
        summary.get("vaults").and_then(Value::as_u64).unwrap_or_default(),
        summary.get("items").and_then(Value::as_u64).unwrap_or_default()
    );
    Ok(())
}

/// `stado host declare-version TARGET --binary B --version V` — say what a
/// host must run.
///
/// `managed_versions` is the declaration every version verdict is measured
/// against, and nothing wrote it: `host inventory` compared each host's
/// binaries to a field that was empty on all three, so every answer was
/// "undeclared" and the fleet looked fine while running whatever it happened
/// to have. Delivery refuses a version the registry has not declared, so
/// without this command the delivery path could not be reached at all.
pub async fn declare_version(
    target: &str,
    binary: &str,
    version: &str,
    json: bool,
) -> Result<(), CmdError> {
    let binary = crate::deploy::host_release::managed_binary(binary)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let version = version.trim();
    if version.is_empty() {
        return Err(CmdError::usage("--version must name an exact version"));
    }
    let (mut document, expected_generation) =
        super::registry::fetch_versioned_document().await?;
    let targets = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?;
    let entry = targets
        .iter_mut()
        .find_map(|candidate| {
            let object = candidate.as_object_mut()?;
            (object.get("name").and_then(Value::as_str) == Some(target)).then_some(object)
        })
        .ok_or_else(|| CmdError::click(format!("registry declares no target {target:?}")))?;
    let versions = entry
        .entry("managed_versions".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| CmdError::click("managed_versions is not an object"))?;
    versions.insert(binary.name.to_string(), json!(version));
    let generation =
        super::registry::push_document_if(&document, &expected_generation).await?;
    if json {
        print_json(&json!({
            "target": target,
            "binary": binary.name,
            "version": version,
            "generation": generation,
        }));
        return Ok(());
    }
    println!("{target}: {} declared at {version}", binary.name);
    Ok(())
}

/// Promote one published version into fleet desired state in one fenced
/// registry write. Every platform manifest must already exist and identify
/// the canonical coordinate before `managed_versions` moves.
pub async fn promote_version(binary: &str, version: &str, json_output: bool) -> Result<(), CmdError> {
    let managed = crate::deploy::host_release::managed_binary(binary)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let version = version.trim();
    if !crate::deploy::host_release::is_exact_semver(version) {
        return Err(CmdError::usage(
            "--version must name an exact immutable semantic version",
        ));
    }
    crate::cli::storage::release_api_origin()?;
    let (mut document, expected_generation) =
        super::registry::fetch_versioned_document().await?;
    let target_specs: Vec<(String, String)> = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?
        .iter()
        .map(|target| {
            let object = target
                .as_object()
                .ok_or_else(|| CmdError::click("registry target must be an object"))?;
            let name = object
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| CmdError::click("registry target has no name"))?;
            let platform = match object.get("release_platform") {
                None | Some(Value::Null) => "",
                Some(Value::String(platform)) => platform.as_str(),
                Some(_) => {
                    return Err(CmdError::click(format!(
                        "registry target {name:?} has a non-string release_platform"
                    )));
                }
            };
            Ok((name.to_string(), platform.to_string()))
        })
        .collect::<Result<_, CmdError>>()?;
    if target_specs.is_empty() {
        return Err(CmdError::click(
            "registry has no targets; refusing an empty desired-state promotion",
        ));
    }

    // Resolve every legacy omission before mutating the in-memory document.
    // A failed channel, malformed inventory, unsupported observation, or
    // disagreement with an existing declaration aborts the one fenced write.
    let runner = crate::deploy::production_runner();
    let mut observed_platforms = std::collections::BTreeMap::new();
    let mut platforms = std::collections::BTreeSet::new();
    let mut migrated = Vec::new();
    for (name, declared) in &target_specs {
        let report = crate::deploy::host_inventory::inventory_host(name, &runner)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "cannot verify release_platform for {name:?}: {error}"
                ))
            })?;
        if report.get("status").and_then(Value::as_str)
            != Some(crate::deploy::host_inventory::OK_STATUS)
        {
            let detail = report
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("host inventory did not complete");
            return Err(CmdError::click(format!(
                "cannot verify release_platform for {name:?}: {detail}"
            )));
        }
        if report.get("sanitizer_state").and_then(Value::as_str)
            != Some(crate::deploy::host_inventory::SANITIZER_OK)
        {
            return Err(CmdError::click(format!(
                "cannot verify release_platform for {name:?}: host inventory sanitizer failed"
            )));
        }
        let observed = report
            .get("release_platform")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "cannot verify release_platform for {name:?}: inventory omitted it"
                ))
            })?;
        let observed = crate::deploy::host_release::managed_platform(observed)
            .map_err(|error| CmdError::click(format!("{name}: {error}")))?;
        if !declared.is_empty() && declared != observed {
            return Err(CmdError::click(format!(
                "registry target {name:?} declares release_platform {declared}, \
                 but verified inventory observed {observed}"
            )));
        }
        if declared.is_empty() {
            migrated.push(name.clone());
        }
        observed_platforms.insert(name.clone(), observed.to_string());
        platforms.insert(observed.to_string());
    }
    for platform in &platforms {
        crate::deploy::host_release::catalog_identity(managed, version, platform)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    }

    let targets = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry.targets: must be an array"))?;
    for target in targets {
        let object = target
            .as_object_mut()
            .ok_or_else(|| CmdError::click("registry target must be an object"))?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| CmdError::click("registry target has no name"))?
            .to_string();
        let observed = observed_platforms.get(&name).ok_or_else(|| {
            CmdError::click(format!("target {name:?} was not inventoried before promotion"))
        })?;
        if object
            .get("release_platform")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .is_empty()
        {
            object.insert("release_platform".to_string(), json!(observed));
        }
        let versions = object
            .entry("managed_versions".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()))
            .as_object_mut()
            .ok_or_else(|| CmdError::click("managed_versions is not an object"))?;
        versions.insert(managed.name.to_string(), json!(version));
    }
    let generation =
        super::registry::push_document_if(&document, &expected_generation).await?;
    if json_output {
        print_json(&json!({
            "binary": managed.name,
            "version": version,
            "targets": target_specs.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            "platforms": platforms,
            "migrated_release_platforms": migrated,
            "generation": generation,
        }));
    } else {
        println!(
            "{} {version} promoted to {} target(s), migrated {} release platform(s), \
             generation {generation}",
            managed.name,
            target_specs.len(),
            migrated.len(),
        );
    }
    Ok(())
}

/// `stado host reconcile [TARGET] [--apply]` — what the fleet runs against
/// what it was told to run.
///
/// Without `--apply` nothing changes: an operator must be able to see drift
/// without a machine moving under them. With it, every host that is BEHIND
/// its declaration is delivered through the ordinary `host release` path,
/// which verifies the digest before it repoints anything.
///
/// Only `behind` is delivered. A host running something NEWER than the
/// declaration is a stale declaration, not a stale host, and quietly
/// downgrading it would be this command destroying work rather than
/// reconciling it.
pub async fn reconcile(
    target: Option<String>,
    apply: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let names: Vec<String> = match target {
        Some(name) => {
            if registry.targets.iter().all(|entry| entry.name != name) {
                return Err(CmdError::click(format!(
                    "registry declares no target {name:?}"
                )));
            }
            vec![name]
        }
        None => registry.targets.iter().map(|entry| entry.name.clone()).collect(),
    };
    if names.is_empty() {
        return Err(CmdError::click("registry has no targets to reconcile"));
    }

    let mut standings = Vec::with_capacity(names.len());
    for name in &names {
        standings.push(crate::deploy::reconcile::examine(name, &runner).await);
    }

    let mut deliveries: Vec<Value> = Vec::new();
    if apply {
        for standing in &standings {
            if !standing.needs_delivery() {
                continue;
            }
            let entry = registry
                .targets
                .iter()
                .find(|entry| entry.name == standing.target)
                .ok_or_else(|| {
                    CmdError::click(format!("registry target {:?} disappeared", standing.target))
                })?;
            for drifted in &standing.drift {
                if drifted.verdict != "behind" && drifted.verdict != "absent" {
                    continue;
                }
                let binary = &drifted.binary;
                let version = entry.declared_version(binary).ok_or_else(|| {
                    CmdError::click(format!(
                        "{} has no desired {binary} version",
                        standing.target
                    ))
                })?;
                let outcome = crate::deploy::host_release::release_host(
                    &standing.target,
                    binary,
                    version,
                    false,
                    &runner,
                )
                .await;
                deliveries.push(match outcome {
                    Ok(report)
                        if matches!(
                            report.get("status").and_then(Value::as_str),
                            Some(
                                crate::deploy::host_release::RELEASED_STATUS
                                    | crate::deploy::host_release::ALREADY_ACTIVE_STATUS
                            )
                        ) =>
                    {
                        json!({
                            "target": standing.target,
                            "binary": binary,
                            "version": version,
                            "status": "delivered",
                            "report": report,
                        })
                    }
                    Ok(report) => json!({
                        "target": standing.target,
                        "binary": binary,
                        "version": version,
                        "status": "failed",
                        "detail": report
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("delivery returned a non-success report"),
                        "report": report,
                    }),
                    Err(error) => json!({
                        "target": standing.target,
                        "binary": binary,
                        "version": version,
                        "status": "failed",
                        "detail": error.to_string(),
                    }),
                });
            }
        }
        standings.clear();
        for name in &names {
            standings.push(crate::deploy::reconcile::examine(name, &runner).await);
        }
    }

    let healthy = standings.iter().all(crate::deploy::reconcile::HostStanding::settled)
        && deliveries
            .iter()
            .all(|entry| entry.get("status").and_then(Value::as_str) == Some("delivered"));
    let report = crate::deploy::reconcile::report(&standings, &deliveries);
    if json_output {
        print_json(&report);
    } else {
        for standing in &standings {
            if let Some(detail) = &standing.unreachable {
                println!("{}: unreachable — {detail}", standing.target);
                continue;
            }
            if standing.platform_verdict != crate::deploy::host_inventory::MATCHED {
                println!(
                    "{}: platform mismatch — declared {}, observed {}",
                    standing.target,
                    standing.declared_release_platform,
                    standing.release_platform
                );
            }
            if standing.settled() {
                println!("{}: active versions match desired state", standing.target);
            }
            for drift in &standing.drift {
                println!(
                    "{}: {} is {} — desired {}, active {}",
                    standing.target,
                    drift.binary,
                    drift.verdict,
                    drift.declared,
                    drift.installed
                );
            }
            if !standing.undeclared.is_empty() {
                println!(
                    "{}: missing desired versions — {}",
                    standing.target,
                    standing.undeclared.join(", ")
                );
            }
        }
        for delivery in &deliveries {
            println!(
                "{} {} on {}: {}",
                delivery.get("binary").and_then(Value::as_str).unwrap_or(""),
                delivery.get("version").and_then(Value::as_str).unwrap_or(""),
                delivery.get("target").and_then(Value::as_str).unwrap_or(""),
                delivery.get("status").and_then(Value::as_str).unwrap_or("")
            );
        }
    }
    if !healthy {
        return Err(CmdError::click(
            "reconcile incomplete: every target must be reachable, platform-matched, declared, \
             and active at its desired versions",
        ));
    }
    Ok(())
}
pub async fn inventory(target: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_inventory::inventory_host(target, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let expected = crate::deploy::host_inventory::OK_STATUS;
    if json {
        print_json(&report);
        return report_outcome(&report, expected);
    }
    let section = |key: &str| {
        report
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };

    println!("target:   {}", cell(report.get("target")));
    // Said before the tables, not after them, because it decides whether the
    // strings in those tables mean anything. A host whose sanitizer does not
    // work reports blank names, and a table of blanks reads like a host with
    // nothing installed on it.
    let sanitizer = report.get("sanitizer_state");
    if sanitizer.and_then(Value::as_str) != Some(crate::deploy::host_inventory::SANITIZER_OK) {
        println!(
            "sanitizer: {} — the host's own field sanitizer failed its probe, so \
             every name, mode, version and URL below is unreliable",
            cell(sanitizer)
        );
    }
    super::table::print(
        &[
            "BINARY",
            "STATE",
            "EXECUTABLE",
            "VERSION STATE",
            "VERSION",
            "DECLARED",
            "VERDICT",
        ],
        &section("managed_binaries")
            .iter()
            .map(|binary| {
                vec![
                    cell(binary.get("name")),
                    cell(binary.get("state")),
                    cell(binary.get("executable")),
                    cell(binary.get("version_state")),
                    cell(binary.get("version")),
                    cell(binary.get("declared_version")),
                    cell(binary.get("version_verdict")),
                ]
            })
            .collect::<Vec<Vec<String>>>(),
    );

    let markers = section("forwards");
    if markers.is_empty() {
        println!(
            "\nforward markers: none ($HOME/.stado/forwards is {})",
            cell(report.get("forwards_dir_state"))
        );
    } else {
        // Two verdict columns because the marker is reconciled against two
        // independent things: LISTENING is whether anything answers where
        // the marker points, DECLARATION is whether the registry sends
        // consumers to the same place. A marker can pass one and fail the
        // other, and collapsing them would hide exactly that case.
        super::table::print(
            &[
                "MARKER",
                "STATE",
                "URL",
                "PORT",
                "LISTENING",
                "DECLARED URL",
                "DECLARATION",
            ],
            &markers
                .iter()
                .map(|marker| {
                    vec![
                        cell(marker.get("name")),
                        cell(marker.get("state")),
                        cell(marker.get("url")),
                        cell(marker.get("port")),
                        cell(marker.get("reconciliation")),
                        cell(marker.get("declared_url")),
                        cell(marker.get("declaration_verdict")),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }

    let listeners = section("listeners");
    super::table::print(
        &["PORT", "PID", "ADDRESS"],
        &listeners
            .iter()
            .map(|listener| {
                vec![
                    cell(listener.get("port")),
                    cell(listener.get("pid")),
                    cell(listener.get("address")),
                ]
            })
            .collect::<Vec<Vec<String>>>(),
    );
    let listeners_state = report.get("listeners_state");
    if listeners_state.and_then(Value::as_str)
        != Some(crate::deploy::host_inventory::LISTENERS_READ)
    {
        // An empty table above and this line missing would read as "nothing
        // is listening on this host", which is the opposite of what happened.
        println!(
            "listeners: {} — the kernel socket table could not be read, so no \
             marker above could be checked against it",
            cell(listeners_state)
        );
    }
    if !listeners.is_empty() {
        // Say what the pid column is NOT, once, where it is being read.
        println!(
            "Owners are pids. Map one to a program with stado host exec {target} \
             -- ps ax -o pid -o ppid -o etime -o comm; this command never reads \
             process arguments or environments."
        );
    }

    super::table::print(
        &["SUBCOMMAND", "INSTALLED BINARY"],
        &section("subcommands")
            .iter()
            .map(|subcommand| vec![cell(subcommand.get("name")), cell(subcommand.get("state"))])
            .collect::<Vec<Vec<String>>>(),
    );

    let vaults = section("vaults");
    if vaults.is_empty() {
        println!("\nvaults: none — $HOME/.stado holds no *.vault.json");
    } else {
        super::table::print(
            &["VAULT", "STATE", "BYTES", "MODE", "OWNER ONLY"],
            &vaults
                .iter()
                .map(|vault| {
                    vec![
                        cell(vault.get("name")),
                        cell(vault.get("state")),
                        cell(vault.get("bytes")),
                        cell(vault.get("mode")),
                        cell(vault.get("owner_only")),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }
    // Snapshots, pre-migration copies and acquisitions files, kept in their
    // own table on purpose: the active vault is state, a sidecar is history,
    // and editing the wrong one is the mistake this separation prevents.
    let sidecars = section("vault_sidecars");
    if !sidecars.is_empty() {
        super::table::print(
            &["VAULT SIDECAR", "STATE", "BYTES", "MODE", "OWNER ONLY"],
            &sidecars
                .iter()
                .map(|sidecar| {
                    vec![
                        cell(sidecar.get("name")),
                        cell(sidecar.get("state")),
                        cell(sidecar.get("bytes")),
                        cell(sidecar.get("mode")),
                        cell(sidecar.get("owner_only")),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }
    if report.get("vaults_truncated") == Some(&Value::Bool(true))
        || report.get("vault_sidecars_truncated") == Some(&Value::Bool(true))
    {
        println!(
            "$HOME/.stado holds more vault files than this command lists; \
             vaults_seen and vault_sidecars_seen carry the real counts."
        );
    }
    println!(
        "Vault rows are metadata only. This command never opens a vault, so no \
         ciphertext, item id, consumer name or token can appear above."
    );

    // The answer, not the raw tables above it.
    let summary = report.get("reconciliation");
    let counted = |key: &str| cell(summary.and_then(|value| value.get(key)));
    println!(
        "\nreconciliation: {} of {} forward markers matched, {} stale, {} unreadable, \
         {} unjudged",
        counted("matched"),
        counted("markers"),
        counted("stale"),
        counted("unreadable"),
        counted("unknown"),
    );
    let name_list = |key: &str| -> Vec<String> {
        summary
            .and_then(|value| value.get(key))
            .and_then(Value::as_array)
            .map(|names| names.iter().map(|name| cell(Some(name))).collect())
            .unwrap_or_default()
    };
    let stale = name_list("stale_markers");
    if !stale.is_empty() {
        println!(
            "stale markers:  {} — the marker names a port nothing is listening on",
            stale.join(", ")
        );
    }

    // The registry axis, said in words. It is deliberately not folded into
    // the line above: "matched" there means something is listening, and a
    // marker can be listening and still send consumers to a port the
    // directory does not declare — which is the drift that survives every
    // health check.
    println!(
        "declaration:    {} of {} forward markers agree with the registry, {} disagree, \
         {} undeclared",
        counted("declaration_matched"),
        counted("markers"),
        counted("declaration_disagrees"),
        counted("declaration_undeclared"),
    );
    let disagreeing = name_list("disagreeing_markers");
    let undeclared_markers = name_list("undeclared_markers");
    if !disagreeing.is_empty() {
        println!(
            "marker vs registry: {} — the marker points at one endpoint and the \
             service directory declares another for this host; consumers resolving \
             through the directory do not arrive where the marker says",
            disagreeing.join(", ")
        );
    }
    if !undeclared_markers.is_empty() {
        println!(
            "undeclared markers: {} — no service in the directory carries an endpoint \
             for this host under that name, so there is nothing to hold the marker to",
            undeclared_markers.join(", ")
        );
    }

    // The version axis. `undeclared` is printed too: a host nobody declared
    // a version for is not a host that passed a version check.
    let behind = name_list("versions_behind");
    let ahead = name_list("versions_ahead");
    let mismatched = name_list("versions_mismatched");
    let unjudged = name_list("versions_unjudged");
    let undeclared_versions = name_list("versions_undeclared");
    if !behind.is_empty() {
        println!(
            "versions behind: {} — older than registry managed_versions declares \
             for this host",
            behind.join(", ")
        );
    }
    if !ahead.is_empty() {
        println!(
            "versions ahead:  {} — newer than registry managed_versions declares; \
             the declaration is the thing that is stale",
            ahead.join(", ")
        );
    }
    if !mismatched.is_empty() {
        println!(
            "versions differ: {} — installed and declared are not the same string, \
             and one of them is not three numbers, so neither is older",
            mismatched.join(", ")
        );
    }
    if !unjudged.is_empty() {
        println!(
            "versions unjudged: {} — the registry declares a version and the host \
             reported none that could be read",
            unjudged.join(", ")
        );
    }
    if !undeclared_versions.is_empty() {
        println!(
            "versions undeclared: {} — the registry declares no required version for \
             this host, so nothing here was verified against a target state",
            undeclared_versions.join(", ")
        );
    }
    if disagreeing.is_empty()
        && undeclared_markers.is_empty()
        && behind.is_empty()
        && ahead.is_empty()
        && mismatched.is_empty()
        && unjudged.is_empty()
        && undeclared_versions.is_empty()
    {
        // One confirming line, for the same reason the vault section has
        // one: a verified host must read as verified, not as a section that
        // printed nothing.
        println!(
            "declared state: every marker matches the endpoint the registry declares, \
             and every managed binary is at its declared version"
        );
    }
    // The two vault findings, in the human output and not only under --json:
    // a signal an operator has to ask for in JSON is a signal they will miss.
    let not_owner_only = name_list("vaults_not_owner_only");
    let refused = name_list("vaults_refused");
    if !not_owner_only.is_empty() {
        println!(
            "vault perms:    {} — readable past the owner; a vault the group \
             can read is an incident, not cosmetics",
            not_owner_only.join(", ")
        );
    }
    if !refused.is_empty() {
        println!(
            "vaults refused: {} — a symlink or not a regular file, reported \
             rather than followed",
            refused.join(", ")
        );
    }
    if not_owner_only.is_empty() && refused.is_empty() {
        // Say it, rather than printing nothing: a clean host must read as
        // checked, not as a section that quietly had nothing to add.
        println!(
            "vaults:         {} active, {} sidecar — all owner-only, none refused",
            vaults.len(),
            sidecars.len()
        );
    }
    report_outcome(&report, expected)
}

/// `stado host release TARGET --binary NAME --version X.Y.Z` — put one
/// registry-declared managed binary onto TARGET.
///
/// The write counterpart of `host inventory`: that command says a host is
/// behind its declared version, this one closes the gap, and it refuses to
/// do anything the declaration does not already say. `--binary` selects a
/// compile-time entry and never becomes a path; `--version` is an exact
/// immutable coordinate that has to equal what the registry declares.
pub async fn release(
    target: &str,
    binary: &str,
    version: &str,
    dry_run: bool,
    json: bool,
) -> Result<(), CmdError> {
    use crate::deploy::host_release;

    let runner = crate::deploy::production_runner();
    let report = host_release::release_host(target, binary, version, dry_run, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    // Three outcomes are success, and conflating them would be the lie this
    // command exists to avoid: a delivery, a host that already ran the
    // requested version, and a dry run that mutated nothing.
    let expected = match report.get("status").and_then(Value::as_str) {
        Some(host_release::ALREADY_ACTIVE_STATUS) => host_release::ALREADY_ACTIVE_STATUS,
        Some(host_release::PLANNED_STATUS) if dry_run => host_release::PLANNED_STATUS,
        _ => host_release::RELEASED_STATUS,
    };
    if json {
        print_json(&report);
        return report_outcome(&report, expected);
    }

    println!("target:   {}", cell(report.get("target")));
    println!(
        "binary:   {} {} ({})",
        cell(report.get("binary")),
        cell(report.get("version")),
        cell(report.get("platform"))
    );
    println!("declared: {}", cell(report.get("declared_version")));
    println!("artifact: {}", cell(report.get("release_uri")));
    println!("sha256:   {} (release manifest)", cell(report.get("sha256")));
    println!(
        "installed: {} ({})",
        cell(report.get("active_version")),
        cell(report.get("active_state"))
    );
    println!("unit:     {}", cell(report.get("unit")));

    let steps = report
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    super::table::print(
        &["STEP", "STATE", "DETAIL"],
        &steps
            .iter()
            .map(|step| {
                vec![
                    cell(step.get("step")),
                    cell(step.get("state")),
                    cell(step.get("detail")),
                ]
            })
            .collect::<Vec<Vec<String>>>(),
    );

    match report.get("status").and_then(Value::as_str) {
        Some(host_release::ALREADY_ACTIVE_STATUS) => println!(
            "\nalready active: {} is the running version, so nothing was fetched, \
             staged, activated or restarted",
            cell(report.get("version"))
        ),
        Some(host_release::PLANNED_STATUS) => {
            // Named one by one, because the value of a dry run is the order.
            println!("\nplanned, nothing was mutated on the host:");
            for step in report
                .get("planned_steps")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
            {
                println!("  {}", cell(Some(step)));
            }
        }
        Some(host_release::RELEASED_STATUS) => println!(
            "\nreleased: {} now runs {} {}",
            cell(report.get("target")),
            cell(report.get("binary")),
            cell(report.get("version"))
        ),
        _ => {
            // The question an operator asks after a failure is what is
            // running now, and the answer is almost always "the same thing
            // as before". Say so rather than making them re-run inventory.
            if report.get("active_version_unchanged") == Some(&Value::Bool(true)) {
                println!(
                    "\nnothing was activated: {} still runs {}",
                    cell(report.get("target")),
                    cell(report.get("active_version"))
                );
            }
        }
    }
    report_outcome(&report, expected)
}

/// `stado host install-helper TARGET SOURCE NAME` — transfer one bounded,
/// owner-executable operator helper without opening an arbitrary remote shell.
pub async fn install_helper(
    target: &str,
    source: &str,
    name: &str,
    json: bool,
) -> Result<(), CmdError> {
    if name.is_empty()
        || name
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')))
    {
        return Err(CmdError::usage(
            "helper name must be a non-empty basename containing only letters, digits, '.', '_' or '-'",
        ));
    }
    let bytes = std::fs::read(source)?;
    if bytes.is_empty() || bytes.len() > usize::from(u16::MAX) {
        return Err(CmdError::click(
            "host helper must contain between one and 65535 bytes",
        ));
    }
    let payload = STANDARD.encode(&bytes);
    let remote_name = crate::deploy::shlex_quote(name);
    let script = format!(
        r#"set -euo pipefail
name={remote_name}
case "$name" in
  ""|*[!A-Za-z0-9._-]*) printf '%s\n' 'invalid helper name' >&2; exit 1 ;;
esac
dir="$HOME/.stado/bin"
tmp="$dir/.${{name}}.stado-install.$$"
trap 'rm -f "$tmp"' EXIT
/bin/mkdir -p "$dir"
/bin/chmod 700 "$dir"
if [ "$(/usr/bin/uname -s)" = "Darwin" ]; then decode=-D; else decode=--decode; fi
printf '%s' '{payload}' | /usr/bin/base64 "$decode" > "$tmp"
/bin/chmod 700 "$tmp"
/bin/mv "$tmp" "$dir/$name"
trap - EXIT
printf '%s\n' "$dir/$name"
"#
    );
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let output = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target}: helper installation failed: {}",
            crate::deploy::host_channel::last_error_line(&output, "remote helper write failed")
        )));
    }
    let path = format!("$HOME/.stado/bin/{name}");
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "source": source,
                "path": path,
                "bytes": bytes.len(),
                "status": "installed",
            }))?
        );
    } else {
        println!("{target}: installed {path} ({} bytes)", bytes.len());
    }
    Ok(())
}

/// Where a delivered file lands, relative to the target account's home.
///
/// Separate from `.stado` itself so a delivery can never take the name of a
/// credential, a helper, or anything else Stado keeps there.
const DELIVERED_FILES_DIR: &str = ".stado/files";

/// `stado host install-file TARGET SOURCE NAME [--executable]` — deliver one
/// file of any size to a registry host through the approved channel.
///
/// The gap this closes: `install-helper` caps a delivery at what fits inside a
/// script, and `install-secret` is for credentials and lands them unreadable
/// and unexecutable by design. Anything else — a built binary, a bundle, a
/// configuration file an operator produced elsewhere — had no channel at all,
/// which is how a private `scp` ends up standing in for the audited one.
pub async fn install_file(
    target: &str,
    source: &str,
    name: &str,
    executable: bool,
    json: bool,
) -> Result<(), CmdError> {
    let metadata = std::fs::symlink_metadata(source)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CmdError::usage("file source must be a regular file"));
    }
    let mode = if executable { "u=rwx,go=" } else { "u=rw,go=" };
    let (path, byte_count) = stream_file(target, source, name, DELIVERED_FILES_DIR, mode).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "source": source,
                "path": path,
                "bytes": byte_count,
                "mode": mode,
                "integrity": "sha256",
                "status": "installed",
            }))?
        );
    } else {
        println!("{target}: installed {path} ({byte_count} bytes, {mode})");
    }
    Ok(())
}
/// Transfer one opaque owner credential without exposing it in argv, stdout,
/// logs, a remote environment variable, or a general-purpose remote shell.
pub async fn install_secret(
    target: &str,
    source: &str,
    name: &str,
    json: bool,
) -> Result<(), CmdError> {
    let metadata = std::fs::symlink_metadata(source)?;
    let unsafe_bits = u32::from_str_radix("077", u8::BITS).unwrap_or_default();
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.mode() & unsafe_bits != u32::default()
    {
        return Err(CmdError::usage(
            "secret source must be a regular owner-only file without group or other permission bits",
        ));
    }
    // Embedding the payload in the script is what caps an inline transfer, not
    // anything about the secret itself, so a file too large to embed is streamed
    // instead of refused. Both paths land owner-only and are checksummed on the
    // far side before they take the name.
    let (path, byte_count) = if metadata.len() > u64::from(u16::MAX) {
        stream_file(target, source, name, ".stado", "u=rw,go=").await?
    } else {
        let bytes = std::fs::read(source)?;
        transfer_secret(target, name, &bytes, None).await?
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "source": source,
                "path": path,
                "bytes": byte_count,
                "integrity": "sha256",
                "status": "installed",
            }))?
        );
    } else {
        println!("{target}: installed owner-only {path} ({byte_count} bytes)");
    }
    Ok(())
}

/// Resolve one exact credential field through Stado's selected store and
/// transfer it directly to a host. The value never reaches argv, stdout, a
/// local temporary file, or the JSON report.
pub async fn install_credential(
    target: &str,
    item: &str,
    field: &str,
    name: &str,
    home: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let value = crate::credential_store::read_string(item, field)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "credential item {item:?} has no string field {field:?}"
            ))
        })?;
    let (path, byte_count) = transfer_secret(target, name, value.as_bytes(), home).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "credential": item,
                "field": field,
                "path": path,
                "bytes": byte_count,
                "integrity": "sha256",
                "status": "installed",
            }))?
        );
    } else {
        println!("{target}: installed {item}.{field} as owner-only {path} ({byte_count} bytes)");
    }
    Ok(())
}

pub(crate) async fn install_secret_value_at_home(
    target: &str,
    name: &str,
    value: &str,
    home: &str,
) -> Result<(String, usize), CmdError> {
    transfer_secret(target, name, value.as_bytes(), Some(home)).await
}

/// Deliver one file through the [`install_file`] channel and RETURN where it
/// landed, for a caller that renders its own report.
///
/// A callee that prints is unusable from a machine-readable caller: with
/// [`install_file`] itself, `stado host publish-placement-policy --json` would
/// put a delivery report in front of its own document and hand the operator two
/// JSON objects on one stream. Same channel, same checksum, same owner-only
/// mode — only the reporting belongs to whoever asked.
pub(crate) async fn deliver_file(
    target: &str,
    source: &str,
    name: &str,
) -> Result<(String, usize), CmdError> {
    stream_file(target, source, name, DELIVERED_FILES_DIR, "u=rw,go=").await
}

async fn transfer_secret(
    target: &str,
    name: &str,
    bytes: &[u8],
    home: Option<&str>,
) -> Result<(String, usize), CmdError> {
    release_component("secret file name", name)?;
    if bytes.is_empty() || bytes.len() > usize::from(u16::MAX) {
        return Err(CmdError::click(
            "host secret must contain between one and 65535 bytes",
        ));
    }
    let mut digest = Sha256::new();
    digest.update(bytes);
    let expected_sha256 = hex::encode(digest.finalize());
    let payload = STANDARD.encode(bytes);
    let remote_name = crate::deploy::shlex_quote(name);
    let remote_expected = crate::deploy::shlex_quote(&expected_sha256);
    let remote_home = match home {
        Some(home) => {
            let valid = home.starts_with('/')
                && !home.chars().any(char::is_control)
                && !home
                    .split('/')
                    .any(|component| matches!(component, "." | ".."));
            if !valid {
                return Err(CmdError::usage(
                    "target home must be an absolute path without '.' or '..' components",
                ));
            }
            crate::deploy::shlex_quote(home)
        }
        None => "\"$HOME\"".to_string(),
    };
    let script = format!(
        r#"set -euo pipefail
name={remote_name}
expected={remote_expected}
home={remote_home}
case "$name" in
  ""|*[!A-Za-z0-9._-]*) printf '%s\n' 'invalid secret file name' >&2; exit 1 ;;
esac
if [ ! -d "$home" ]; then
  printf '%s\n' 'target home directory does not exist' >&2
  false
fi
os=$(/usr/bin/uname -s)
if [ "$os" = "Darwin" ]; then
  decode=-D
  owner=$(/usr/bin/stat -f %Su "$home")
  group=$(/usr/bin/stat -f %Sg "$home")
else
  decode=--decode
  owner=$(/usr/bin/stat -c %U "$home")
  group=$(/usr/bin/stat -c %G "$home")
fi
if [ -x /usr/bin/chown ]; then chown_bin=/usr/bin/chown; else chown_bin=/usr/sbin/chown; fi
current=$(/usr/bin/id -un)
if [ "$owner" != "$current" ] && [ "$(/usr/bin/id -u)" -ne 0 ]; then
  printf '%s\n' 'SSH account cannot write the selected target home' >&2
  false
fi
dir="$home/.stado"
tmp="$dir/.${{name}}.stado-secret.$$"
trap 'rm -f "$tmp"' EXIT
/bin/mkdir -p "$dir"
/bin/chmod 700 "$dir"
if [ "$owner" != "$current" ]; then "$chown_bin" "$owner:$group" "$dir"; fi
printf '%s' '{payload}' | /usr/bin/base64 "$decode" > "$tmp"
/bin/chmod 600 "$tmp"
if [ "$owner" != "$current" ]; then "$chown_bin" "$owner:$group" "$tmp"; fi
/bin/mv "$tmp" "$dir/$name"
line=$(/usr/bin/openssl dgst -sha256 -r "$dir/$name")
actual="${{line%% *}}"
if [ "$actual" != "$expected" ]; then
  printf '%s\n' 'secret transfer checksum mismatch' > /dev/stderr
  false
fi
trap - EXIT
printf '%s\n' "$dir/$name"
"#
    );
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let output = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target}: secret installation failed: {}",
            crate::deploy::host_channel::last_error_line(&output, "remote secret write failed")
        )));
    }
    Ok((
        format!("{}/.stado/{name}", home.unwrap_or("$HOME")),
        bytes.len(),
    ))
}

/// The shape of a UUID, written out rather than counted: every `x` is one hex digit
/// and the dashes fall where they fall. A template compares as exactly as a length
/// arithmetic would and stays legible at the callsite.
const UUID_SHAPE: &str = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx";

/// Is this string a UUID, and therefore free of anything a shell could act on?
fn is_uuid(value: &str) -> bool {
    value.len() == UUID_SHAPE.len()
        && value
            .bytes()
            .zip(UUID_SHAPE.bytes())
            .all(|(byte, shape)| match shape {
                b'-' => byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            })
}

/// Run one helper previously placed in the remote owner-only Stado directory.
///
/// Arguments are accepted only as UUIDs. That is not a stylistic limit: the reason
/// this refused every argument was that operator words become a shell escape, and a
/// UUID cannot be a path, a flag, a glob, a redirection or a metacharacter -- there is
/// nothing in the grammar to escape with. The helper stays the reviewed program; the
/// UUID only tells it which of the operator's own records to act on.
///
/// Refusing outright is what pushed callers into private ssh invocations with their
/// own key files and their own known_hosts, which is the same action with the audit
/// trail removed. A correlation id is the smallest thing that lets those callers come
/// back through the registry channel.
pub async fn run_helper(
    target: &str,
    name: &str,
    uuids: &[String],
    json: bool,
) -> Result<(), CmdError> {
    release_component("helper name", name)?;
    let mut arguments = String::new();
    for uuid in uuids {
        if !is_uuid(uuid) {
            return Err(CmdError::click(format!(
                "{uuid:?} is not a UUID; `host run-helper` carries correlation \
                 identifiers and nothing else, because anything with shell grammar in \
                 it would make the helper a remote shell"
            )));
        }
        arguments.push(' ');
        arguments.push_str(uuid);
    }
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let remote_name = crate::deploy::shlex_quote(name);
    let script =
        crate::deploy::host_channel::installed_helper_script(&remote_name, &arguments);
    let runner = crate::deploy::production_runner();
    let output = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        &script,
        std::time::Duration::from_secs(crate::monitor::billing::SECONDS_PER_HOUR),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "helper": name,
                "status": if output.ok() { "completed" } else { "failed" },
                "exit_code": output.code,
                "stdout": output.stdout,
                "stderr": output.stderr,
            }))?
        );
    } else {
        print!("{}", output.stdout);
        eprint!("{}", output.stderr);
    }
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target}: helper {name} failed: {}",
            crate::deploy::host_channel::last_error_line(&output, "remote helper failed")
        )));
    }
    Ok(())
}

/// The exact removal `host remove-helper` performs, for one named helper on an
/// already-resolved target.
///
/// Shared with `host helpers --prune`, which removes many of these in one run.
/// One function, because two spellings of "delete a file under `.stado/bin`"
/// would be two policies about symlinks, about what counts as absent, and
/// about which channel the deletion is audited on -- and the second one always
/// turns out to be a shell one-liner somebody wrote in a hurry.
///
/// Returns the remote's own word: `removed` or `absent`. Absent is not a
/// failure; a helper listed by an inventory taken seconds ago and gone by the
/// time the removal lands is a race with a truthful outcome, and reporting it
/// as an error would send an operator looking for a fault nobody committed.
async fn remove_installed_helper(
    resolved: &ComputeTarget,
    name: &str,
    runner: &crate::deploy::Runner,
) -> Result<String, CmdError> {
    // The name may have come back from a remote inventory rather than from the
    // operator's own argv, so it is checked here as well: this is the last
    // place before it is interpolated into a script that deletes files.
    release_component("helper name", name)?;
    let remote_name = crate::deploy::shlex_quote(name);
    // Regular file only: a symlink under that name is not something this
    // command put there, and following it would delete an unrelated path.
    let script = format!(
        r#"set -euo pipefail
helper="$HOME/.stado/bin/"{remote_name}
if [ -L "$helper" ]; then
  printf '%s\n' "refusing to remove symlink: $helper" > /dev/stderr
  false
elif [ -f "$helper" ]; then
  rm -f -- "$helper"
  printf '%s\n' removed
else
  printf '%s\n' absent
fi
"#
    );
    let output = crate::deploy::host_channel::run_script_with_timeout(
        resolved,
        &script,
        std::time::Duration::from_secs(crate::monitor::billing::SECONDS_PER_HOUR),
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: helper {name} could not be removed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&output, "remote removal failed")
        )));
    }
    Ok(output.stdout.trim().to_string())
}

/// Remove one previously installed helper. Installing a helper is how Stado
/// runs what the exec allowlist refuses, which makes every diagnostic a file
/// left behind on someone else's machine; without this the fleet accumulates
/// them and nothing but the operator's memory says what they were for.
pub async fn remove_helper(target: &str, name: &str, json: bool) -> Result<(), CmdError> {
    release_component("helper name", name)?;
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let state = remove_installed_helper(&resolved, name, &runner).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "helper": name,
                "status": state,
            }))?
        );
    } else {
        println!("{target}: {name} {state}");
    }
    Ok(())
}

/// The helper that inventories `$HOME/.stado/bin` on a target.
///
/// Installed with
/// `stado host install-helper <target> stado-rs/scripts/report-stale-helpers.sh
/// report-stale-helpers`.
const INVENTORY_HELPER: &str = "report-stale-helpers";

/// One installed helper script, as the remote inventory reported it.
struct InstalledHelper {
    name: String,
    bytes: u64,
    /// Seconds since the file was last written, from the remote's own clock
    /// against this one. Skew shows up as a small negative that clamps to
    /// zero, which is a helper installed moments ago -- exactly what it is.
    age_seconds: i64,
}

/// One `name<TAB>mtime<TAB>size` line from `report-stale-helpers`.
///
/// `None` for anything that is not exactly those three fields. A short line, a
/// number that will not parse, a fourth field from a newer helper: none of
/// them is a helper this can date or size, and inventing a zero for the
/// missing half would put a fabricated row at the head of an oldest-first list
/// -- which is the first thing `--prune` would then delete.
fn parse_inventory_line(line: &str, now: i64) -> Option<InstalledHelper> {
    let mut fields = line.split('\t');
    let name = fields.next().filter(|name| !name.is_empty())?;
    let modified: i64 = fields.next()?.parse().ok()?;
    let bytes: u64 = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some(InstalledHelper {
        name: name.to_string(),
        bytes,
        // Clamped: a remote clock ahead of this one is skew, not a helper
        // installed in the future, and a negative age would sort to the end of
        // the list and read as the newest thing on the host.
        age_seconds: (now - modified).max(i64::default()),
    })
}

/// A prune that could not remove everything it named, reported after the
/// table rather than instead of it.
///
/// The rows already printed are true and the operator needs them more than
/// they need an early exit; the non-zero status is what stops a caller
/// treating a partial sweep as a completed one.
fn removal_outcome(target: &str, failed: usize) -> Result<(), CmdError> {
    if failed == usize::default() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{target}: {failed} helper(s) could not be removed; each is reported above with the \
         remote's own words"
    )))
}

/// `stado host helpers TARGET [--older-than-days N] [--prune] [--json]` — every
/// helper script this host carries, oldest first.
///
/// `install-helper` has a writer and no reaper. control-host carries 553
/// installed helper scripts beside 16 binaries: each was delivered to settle
/// one incident, none was ever withdrawn, and `host provenance` can only print
/// the count as a footnote because nothing enumerated them. This is the
/// enumeration -- name, age and size, which is the least an operator needs to
/// decide whether a script from an incident nobody remembers may go.
///
/// Removal is under `--prune` and never otherwise, and `--prune` demands an
/// explicit `--older-than-days`: a sweep with no threshold means "remove
/// everything", which is never the intent on a directory whose 553 entries
/// include the ones three products currently run.
///
/// The inventory comes from the installed helper rather than from a shell
/// one-liner, and every removal goes back through the same audited channel
/// `host remove-helper` uses, one named helper at a time. A host without the
/// inventory helper is an error naming the install command, never an empty
/// table: "nobody looked" rendered as "nothing is there" is the fold this
/// fleet has already paid for.
pub async fn helpers(
    target: &str,
    older_than_days: Option<u32>,
    prune: bool,
    json: bool,
) -> Result<(), CmdError> {
    if prune && older_than_days.is_none() {
        return Err(CmdError::usage(
            "--prune requires --older-than-days: removing every helper is never the intent, \
             and this directory holds the scripts three products currently run",
        ));
    }
    let runner = crate::deploy::production_runner();
    let inventory =
        crate::deploy::host_channel::run_installed_helper(target, INVENTORY_HELPER, &runner)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "{target}: cannot read the helper inventory: {error}; install it with \
                     `stado host install-helper {target} \
                     stado-rs/scripts/report-stale-helpers.sh {INVENTORY_HELPER}`"
                ))
            })?;

    let now = chrono::Utc::now().timestamp();
    let mut installed: Vec<InstalledHelper> = Vec::new();
    // A line the inventory format does not explain is carried to the operator
    // rather than dropped: a row this cannot parse is a row this cannot reason
    // about, and silently shrinking the population is how a count stops being
    // evidence.
    let mut unreadable: Vec<String> = Vec::new();
    for line in inventory.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        match parse_inventory_line(line, now) {
            Some(helper) => installed.push(helper),
            None => unreadable.push(line.to_string()),
        }
    }
    // Oldest first: the head of this list is where the fossils are, and it is
    // also exactly what `--prune` acts on, so the two orders cannot disagree.
    installed.sort_by(|left, right| {
        right
            .age_seconds
            .cmp(&left.age_seconds)
            .then_with(|| left.name.cmp(&right.name))
    });

    let threshold = older_than_days.map(|days| {
        i64::from(days).saturating_mul(
            i64::try_from(crate::monitor::billing::SECONDS_PER_DAY).unwrap_or(i64::MAX),
        )
    });
    let stale = |helper: &InstalledHelper| {
        threshold.is_some_and(|threshold| helper.age_seconds >= threshold)
    };

    let mut pruned: Vec<Value> = Vec::new();
    let mut failed = usize::default();
    if prune {
        let resolved = crate::deploy::host_channel::canonical_target(target)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        // One failure does not end the sweep. A helper that is a symlink is
        // refused by the removal script by design, and letting that abort the
        // run would leave the remaining hundreds unreported and the operator
        // with no idea how far it got.
        for helper in installed.iter().filter(|helper| stale(*helper)) {
            match remove_installed_helper(&resolved, &helper.name, &runner).await {
                Ok(status) => pruned.push(json!({"helper": helper.name, "status": status})),
                Err(error) => {
                    failed += 1;
                    pruned.push(json!({
                        "helper": helper.name,
                        "status": "failed",
                        "error": error.to_string(),
                    }));
                }
            }
        }
    }

    let older = installed.iter().filter(|helper| stale(*helper)).count();
    if json {
        let rows: Vec<Value> = installed
            .iter()
            .map(|helper| {
                json!({
                    "helper": helper.name,
                    "age_seconds": helper.age_seconds,
                    "bytes": helper.bytes,
                    "stale": threshold.map(|_| stale(helper)),
                })
            })
            .collect();
        let mut report = json!({
            "target": target,
            "helpers": rows,
            "total": installed.len(),
            "older_than_days": older_than_days,
            "older_than_threshold": older,
            "unreadable": unreadable,
        });
        if prune {
            report["pruned"] = Value::Array(pruned);
        }
        println!("{}", serde_json::to_string_pretty(&report)?);
        return removal_outcome(target, failed);
    }

    for line in &unreadable {
        eprintln!("{target}: an inventory line could not be read: {line}");
    }
    if installed.is_empty() {
        println!("{target}: carries no installed helper scripts");
        return Ok(());
    }
    let rows: Vec<Vec<String>> = installed
        .iter()
        .map(|helper| {
            let mut row = vec![
                helper.name.clone(),
                super::registry::human_age(chrono::TimeDelta::seconds(helper.age_seconds)),
                helper.bytes.to_string(),
            ];
            if threshold.is_some() {
                row.push(if stale(helper) { "yes" } else { "no" }.to_string());
            }
            row
        })
        .collect();
    if threshold.is_some() {
        super::table::print(&["HELPER", "AGE", "BYTES", "OLDER"], &rows);
    } else {
        super::table::print(&["HELPER", "AGE", "BYTES"], &rows);
    }
    for entry in &pruned {
        println!(
            "pruned {}: {}{}",
            entry.get("helper").and_then(Value::as_str).unwrap_or(""),
            entry.get("status").and_then(Value::as_str).unwrap_or(""),
            entry
                .get("error")
                .and_then(Value::as_str)
                .map_or_else(String::new, |error| format!(": {error}"))
        );
    }
    // The count is the finding. One helper left behind is untidy; 553 of them
    // is a directory that accumulated while nobody decided to keep any of it,
    // and only the number says which of those two an operator is looking at.
    println!(
        "\n{target}: {} installed helper script(s) in $HOME/.stado/bin",
        installed.len()
    );
    if let Some(days) = older_than_days {
        println!("{target}: {older} older than {days} day(s)");
        if !prune && older != usize::default() {
            println!("nothing was removed; `--prune` removes exactly those {older}");
        }
    }
    removal_outcome(target, failed)
}

/// Open a background reverse SSH forward using the exact registry channel.
/// Both ends bind loopback; SSH supplies transport encryption and refuses to
/// report success until the remote listener exists.
pub async fn forward_local(
    target: &str,
    name: &str,
    remote_port: u16,
    local_port: u16,
    json: bool,
) -> Result<(), CmdError> {
    if remote_port == u16::default() || local_port == u16::default() {
        return Err(CmdError::usage("forwarding ports must be nonzero"));
    }
    release_component("forward name", name)?;
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if crate::deploy::host_channel::target_is_this_host(&resolved) {
        return Err(CmdError::usage(
            "forward-local requires a remote registry target",
        ));
    }
    let ssh = resolved
        .ssh
        .as_deref()
        .ok_or_else(|| CmdError::click("registry target has no SSH destination"))?;
    let mut argv = crate::deploy::host_channel::ssh_options(ssh);
    let destination = argv
        .pop()
        .ok_or_else(|| CmdError::click("SSH channel has no destination"))?;
    argv.extend([
        "-f".to_string(),
        "-N".to_string(),
        "-o".to_string(),
        "ExitOnForwardFailure=yes".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=30".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
        "-R".to_string(),
        format!("127.0.0.1:{remote_port}:127.0.0.1:{local_port}"),
        destination,
    ]);
    let (program, arguments) = argv
        .split_first()
        .ok_or_else(|| CmdError::click("SSH channel is empty"))?;
    let output = tokio::process::Command::new(program)
        .args(arguments)
        .kill_on_drop(true)
        .output()
        .await?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "{target}: reverse SSH forwarding failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next_back()
                .unwrap_or("ssh forwarding failed")
        )));
    }
    let endpoint = format!("http://127.0.0.1:{remote_port}");
    let marker = format!("$HOME/.stado/forwards/{name}.url");
    let marker_script = format!(
        "set -euo pipefail\ndirectory=\"$HOME/.stado/forwards\"\n/bin/mkdir -p \"$directory\"\n/bin/chmod u=rwx,go= \"$directory\"\nprintf '%s\\n' {endpoint} > \"$directory/\"{name}\".url\"\n/bin/chmod u=rw,go= \"$directory/\"{name}\".url\"\n",
        endpoint = crate::deploy::shlex_quote(&endpoint),
        name = crate::deploy::shlex_quote(name),
    );
    let runner = crate::deploy::production_runner();
    let marked = crate::deploy::host_channel::run_script(&resolved, &marker_script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !marked.ok() {
        return Err(CmdError::click(format!(
            "{target}: forwarding is live but its endpoint marker failed: {}",
            crate::deploy::host_channel::last_error_line(&marked, "remote endpoint marker failed")
        )));
    }
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    let local_marker_directory = std::path::Path::new(&home).join(".stado").join("forwards");
    std::fs::create_dir_all(&local_marker_directory)?;
    let local_marker_path = local_marker_directory.join(format!("{name}.local"));
    std::fs::write(
        &local_marker_path,
        format!("http://127.0.0.1:{local_port}\n"),
    )?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "remote": format!("127.0.0.1:{remote_port}"),
                "local": format!("127.0.0.1:{local_port}"),
                "marker": marker,
                "local_marker": local_marker_path,
                "transport": "ssh",
                "status": "forwarding",
            }))?
        );
    } else {
        println!(
            "{target}: forwarding 127.0.0.1:{remote_port} to local 127.0.0.1:{local_port} over SSH"
        );
    }
    Ok(())
}

fn release_component(kind: &str, value: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(CmdError::usage(format!(
            "{kind} must contain only letters, digits, '.', '_' or '-'"
        )));
    }
    Ok(())
}

fn release_archive_hash(path: &std::path::Path) -> Result<(u64, String), CmdError> {
    let mut file = std::fs::File::open(path)?;
    let bytes = file.metadata()?.len();
    if bytes == u64::default() {
        return Err(CmdError::click("release archive is empty"));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [u8::MIN; u16::MAX as usize];
    loop {
        let read = file.read(&mut buffer)?;
        if read == usize::default() {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok((bytes, hex::encode(hasher.finalize())))
}

/// Transfer one immutable release archive through Stado's registry-authorized
/// SSH channel. The remote side verifies the local digest before create-only
/// publication into its owner-only staging tree.
pub async fn install_release(
    target: &str,
    source: &str,
    family: &str,
    version: &str,
    platform: &str,
    json: bool,
) -> Result<(), CmdError> {
    release_component("family", family)?;
    release_component("version", version)?;
    release_component("platform", platform)?;
    let asset = format!("{family}.tar.gz");
    let source_path = std::path::Path::new(source);
    if !source_path.is_file() || source_path.is_symlink() {
        return Err(CmdError::click(format!(
            "release source must be a regular file: {}",
            source_path.display()
        )));
    }
    let (bytes, sha256) = release_archive_hash(source_path)?;
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let relative = format!(".stado/releases/{family}/{version}/{platform}");
    let temporary = format!(".{asset}.stado-{}", uuid::Uuid::new_v4().simple());
    let prepare = format!(
        "set -euo pipefail\ndirectory=\"$HOME/{relative}\"\n/bin/mkdir -p \"$directory\"\n/bin/chmod u=rwx,go= \"$directory\"\nprintf '%s\\n' \"$HOME\"\n"
    );
    let runner = crate::deploy::production_runner();
    let prepared = crate::deploy::host_channel::run_script(&resolved, &prepare, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !prepared.ok() {
        return Err(CmdError::click(format!(
            "{target}: cannot prepare release staging: {}",
            crate::deploy::host_channel::last_error_line(
                &prepared,
                "remote release directory creation failed"
            )
        )));
    }
    let remote_home = prepared.stdout.trim();
    if !remote_home.starts_with('/')
        || remote_home.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
        })
    {
        return Err(CmdError::click(
            "target returned an unsafe or non-absolute home directory",
        ));
    }
    let remote_temporary = format!("{remote_home}/{relative}/{temporary}");
    let remote_final = format!("{remote_home}/{relative}/{asset}");
    if crate::deploy::host_channel::target_is_this_host(&resolved) {
        std::fs::copy(source_path, &remote_temporary)?;
    } else {
        let ssh = resolved
            .ssh
            .as_deref()
            .ok_or_else(|| CmdError::click("registry target has no SSH destination"))?;
        let key = crate::deploy::ssh_key::materialize(&resolved.name)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let destination = format!("{ssh}:{remote_temporary}");
        let transferred = tokio::process::Command::new("scp")
            .arg("-i")
            .arg(key.path())
            .arg("-o")
            .arg("IdentitiesOnly=yes")
            .arg("-q")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=15")
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new")
            .arg(source_path)
            .arg(&destination)
            .kill_on_drop(true)
            .output()
            .await?;
        if !transferred.status.success() {
            return Err(CmdError::click(format!(
                "{target}: release transfer failed: {}",
                String::from_utf8_lossy(&transferred.stderr)
                    .lines()
                    .next_back()
                    .unwrap_or("scp failed")
            )));
        }
    }
    let verify = format!(
        r#"set -euo pipefail
temporary={temporary}
final={final}
expected={expected}
trap '/bin/rm -f "$temporary"' EXIT
line=$(/usr/bin/openssl dgst -sha256 -r "$temporary")
actual="${{line%% *}}"
if [ "$actual" != "$expected" ]; then
  printf '%s\n' "release checksum mismatch: expected=$expected actual=$actual" > /dev/stderr
  false
fi
if [ -e "$final" ]; then
  line=$(/usr/bin/openssl dgst -sha256 -r "$final")
  existing="${{line%% *}}"
  if [ "$existing" != "$expected" ]; then
    printf '%s\n' "immutable release path already contains a different archive" > /dev/stderr
    false
  fi
  /bin/rm -f "$temporary"
else
  /bin/mv "$temporary" "$final"
fi
/bin/chmod u=rw,go=r "$final"
trap - EXIT
printf '%s\n' "$final"
"#,
        temporary = crate::deploy::shlex_quote(&remote_temporary),
        final = crate::deploy::shlex_quote(&remote_final),
        expected = crate::deploy::shlex_quote(&sha256),
    );
    let verified = crate::deploy::host_channel::run_script(&resolved, &verify, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !verified.ok() {
        return Err(CmdError::click(format!(
            "{target}: release verification failed: {}",
            crate::deploy::host_channel::last_error_line(
                &verified,
                "remote release verification failed"
            )
        )));
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "source": source,
                "family": family,
                "version": version,
                "platform": platform,
                "path": format!("$HOME/{relative}/{asset}"),
                "bytes": bytes,
                "sha256": sha256,
                "status": "installed",
            }))?
        );
    } else {
        println!(
            "{target}: installed {family}/{version}/{platform}/{asset} ({bytes} bytes, sha256={sha256})"
        );
    }
    Ok(())
}

const INSTALL_BODY: &str = r#"dir="$HOME/.stado/bin"
staged="$dir/.$name.install"
installed="$dir/$name"
previous="$dir/$name.previous"
trap 'rm -f "$staged"' EXIT

[ -s "$staged" ] || { printf '%s\n' 'delivered program is missing or empty' >&2; exit 1; }
/bin/chmod 755 "$staged"

if [ "$(/usr/bin/uname -s)" = "Darwin" ]; then
  /usr/bin/xattr -c "$staged" 2>/dev/null || true
  /usr/bin/codesign -s - --force "$staged" >/dev/null 2>&1 \
    || { printf '%s\n' 'delivered program could not be signed on this host' >&2; exit 1; }
fi

new_version="$("$staged" --version 2>&1)" \
  || { printf '%s\n' "delivered program does not run here: $new_version" >&2; exit 1; }

old_version="absent"
if [ -x "$installed" ]; then
  old_version="$("$installed" --version 2>&1 || printf '%s' 'unreadable')"
  /bin/cp -p "$installed" "$previous"
fi

/bin/mv "$staged" "$installed"
trap - EXIT

if ! "$installed" --version >/dev/null 2>&1; then
  if [ -f "$previous" ]; then /bin/mv "$previous" "$installed"; fi
  printf '%s\n' 'installed program does not run; rolled back to the previous build' >&2
  exit 1
fi

printf 'STADO-BIN-OLD %s\n' "$old_version"
printf 'STADO-BIN-NEW %s\n' "$new_version"
"#;

/// Replace an owner-only Stado program on TARGET with a build proven to run there.
///
/// These binaries are what every other operation on that host goes through, so
/// installing one wrongly removes the means of repair. Three rules are encoded
/// here rather than left to whoever is at the keyboard:
///
/// * the new binary is renamed into place, never written through the file that
///   is already there -- overwriting a Mach-O in place invalidates its
///   signature and the kernel answers the next exec with SIGKILL, no message;
/// * it is signed and then executed on the target BEFORE it becomes the CLI,
///   because a binary that is merely present is not evidence of anything;
/// * the previous build is kept, and a version that will not run is rolled
///   back automatically rather than reported as an installation.
pub async fn install_binary(
    target: &str,
    source: Option<&str>,
    name: &str,
    rollback: bool,
    json: bool,
) -> Result<(), CmdError> {
    if name.is_empty()
        || name
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')))
    {
        return Err(CmdError::usage(
            "program name must be a non-empty basename containing only letters, digits, '.', '_' or '-'",
        ));
    }
    if rollback {
        return rollback_binary(target, name, json).await;
    }
    let source = source.ok_or_else(|| CmdError::usage("--from is required unless --rollback"))?;
    let bytes = std::fs::metadata(source)
        .map_err(|error| CmdError::click(format!("cannot read {source}: {error}")))?
        .len();
    if bytes == 0 {
        return Err(CmdError::click(format!("{source} is empty")));
    }

    // Computed before anything is delivered, on this machine, because this is
    // the last moment the answer exists: the checkout the file came out of is
    // here and nowhere else, and once the process ends nothing on either side
    // can reconstruct it.
    let provenance = crate::provenance::describe(std::path::Path::new(source), name);
    if let Some(unknown) = unprovenanced_reason(source, &provenance) {
        eprintln!(
            "{target}: DRIFTED -- installing {name} from a build with no producer: {unknown}. \
             This is the trade that put stado 0.7.1 on control-host, a control-plane \
             binary whose version no commit on any branch of this repository has ever \
             contained, and that left the Weles worker beside it on release main-objapi-fix, \
             built on a laptop and never published -- installing is one command and releasing \
             is a pipeline. The install proceeds, because refusing strands whoever is \
             mid-incident; the manifest at $HOME/{PROVENANCE_DIR}/{name}.json records the gap \
             so `stado host provenance {target}` finds it later without anyone remembering \
             this line."
        );
    }
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let ssh_target = resolved.ssh.clone().unwrap_or_default();
    if ssh_target.is_empty() {
        return Err(CmdError::click(format!(
            "{target} declares no ssh destination, so the CLI cannot be delivered"
        )));
    }
    let runner = crate::deploy::production_runner();

    let stage = crate::deploy::host_channel::run_script(
        &resolved,
        "set -euo pipefail\n/bin/mkdir -p \"$HOME/.stado/bin\"\n/bin/chmod 700 \"$HOME/.stado/bin\"\n",
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !stage.ok() {
        return Err(CmdError::click(format!(
            "{target}: cannot prepare the CLI directory: {}",
            crate::deploy::host_channel::last_error_line(&stage, "remote mkdir failed")
        )));
    }

    // The same command has to work on the machine running it, where there is no
    // ssh listener to talk to and a copy is just a copy.
    if crate::deploy::host_channel::target_is_this_host(&resolved) {
        let home = std::env::var("HOME")
            .map_err(|_| CmdError::click("HOME is not set, so the CLI path is unknown"))?;
        let staged = std::path::Path::new(&home).join(format!(".stado/bin/.{name}.install"));
        std::fs::copy(source, &staged).map_err(|error| {
            CmdError::click(format!(
                "cannot stage the CLI at {}: {error}",
                staged.display()
            ))
        })?;
        return finish_install(
            target,
            source,
            name,
            bytes,
            &provenance,
            &resolved,
            &runner,
            json,
        )
        .await;
    }
    let mut copy_argv = crate::deploy::host_channel::ssh_options(&ssh_target);
    copy_argv.pop();
    let mut scp_argv = vec!["scp".to_string(), "-q".to_string()];
    scp_argv.extend(copy_argv.into_iter().skip(usize::from(true)));
    scp_argv.push(source.to_string());
    scp_argv.push(format!("{ssh_target}:.stado/bin/.{name}.install"));
    let copy = runner(crate::deploy::CommandSpec::new(scp_argv))
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !copy.ok() {
        return Err(CmdError::click(format!(
            "{target}: cannot deliver the CLI: {}",
            copy.detail()
        )));
    }

    finish_install(
        target,
        source,
        name,
        bytes,
        &provenance,
        &resolved,
        &runner,
        json,
    )
    .await
}

/// Sign, prove, swap and verify -- the half of `install-binary` that is identical
/// whether the program arrived over ssh or was copied on the spot.
///
/// The provenance record travels as an argument rather than being recomputed
/// here: it is derived on the machine that holds the checkout, and hashing a
/// release binary twice per install to save a parameter is a poor trade.
#[allow(clippy::too_many_arguments)]
async fn finish_install(
    target: &str,
    source: &str,
    name: &str,
    bytes: u64,
    provenance: &crate::provenance::Provenance,
    resolved: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
    json: bool,
) -> Result<(), CmdError> {
    let quoted = crate::deploy::shlex_quote(name);
    let script = format!("set -euo pipefail\nname={quoted}\n{INSTALL_BODY}");
    let output = crate::deploy::host_channel::run_script(resolved, &script, runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target}: CLI update failed: {}",
            crate::deploy::host_channel::last_error_line(&output, "remote install failed")
        )));
    }
    let marker = |tag: &str| -> String {
        output
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix(tag))
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let old_version = marker("STADO-BIN-OLD ");
    let new_version = marker("STADO-BIN-NEW ");

    // After the swap and before the report: a manifest that arrives first
    // would describe a binary that may never land, and a report printed first
    // would be the same one-line receipt that said `stado 0.7.1 -> stado
    // 0.7.0` and left nothing behind to check it against.
    let manifest = deliver_provenance(target, provenance)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "{target}: {name} is installed but its provenance manifest could not be \
                 delivered, so the host now carries an artifact nothing can trace to a \
                 commit: {error}"
            ))
        })?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "source": source,
                "name": name,
                "bytes": bytes,
                "previous_version": old_version,
                "installed_version": new_version,
                "commit": provenance.commit,
                "sha256": provenance.sha256,
                "builder": provenance.builder,
                "provenance": manifest,
                "status": "installed",
            }))?
        );
    } else {
        println!(
            "{target}: {name} {old_version} -> {new_version} (commit {}, built on {}, recorded at {manifest})",
            provenance.commit, provenance.builder
        );
    }
    Ok(())
}

/// Where a delivered provenance manifest lands, one file per artifact.
///
/// Its own directory under `.stado`, not beside the binary in `.stado/bin`:
/// everything in that directory is executed, and a JSON document that is not
/// a program has no business sharing a namespace with the ones that are.
const PROVENANCE_DIR: &str = ".stado/provenance";

/// Why this build cannot be traced to a published commit, or `None` when it
/// can.
///
/// Three distinct answers rather than one boolean, because they send an
/// operator to three different places: build it inside a checkout, commit
/// what you built, push what you committed. A single "unverified" would have
/// described 0.7.1 accurately and told nobody what to do about it.
fn unprovenanced_reason(
    source: &str,
    provenance: &crate::provenance::Provenance,
) -> Option<String> {
    let Some(repo) = crate::provenance::source_repo(std::path::Path::new(source)) else {
        return Some(format!(
            "{source} does not sit under a git checkout's target/ directory, so no tree \
             claims to have produced it"
        ));
    };
    if !provenance.names_a_commit() {
        return Some(format!(
            "{} has no resolvable HEAD, so the build has no commit to name",
            repo.display()
        ));
    }
    if !crate::provenance::reachable_in_repo(&provenance.commit, &repo) {
        return Some(format!(
            "commit {} is not reachable from origin/main in {}, so it exists only on the \
             machine that built it",
            provenance.commit,
            repo.display()
        ));
    }
    None
}

/// Deliver one artifact's manifest to the host that now carries the artifact.
///
/// Through `stream_file`, the same audited channel the binary itself went
/// over: owner-only, checksummed on the far side before it takes its name. A
/// separate private path for the paperwork would be a second way onto these
/// hosts, which is the shape of the problem this is meant to close.
async fn deliver_provenance(
    target: &str,
    provenance: &crate::provenance::Provenance,
) -> Result<String, CmdError> {
    let document = serde_json::to_vec_pretty(provenance)?;
    let staged = tempfile::Builder::new()
        .prefix(".stado-provenance-")
        .suffix(".json")
        .tempfile()?;
    std::fs::write(staged.path(), &document)?;
    let source = staged
        .path()
        .to_str()
        .ok_or_else(|| CmdError::click("provenance staging path is not valid UTF-8"))?;
    let name = format!("{}.json", provenance.artifact);
    let (path, _) = stream_file(target, source, &name, PROVENANCE_DIR, "u=rw,go=").await?;
    Ok(path)
}

const ROLLBACK_BODY: &str = r#"dir="$HOME/.stado/bin"
installed="$dir/$name"
previous="$dir/$name.previous"

[ -s "$previous" ] || { printf '%s\n' 'there is no previous build to restore' >&2; exit 1; }
/bin/chmod 755 "$previous"
"$previous" --version >/dev/null 2>&1 \
  || { printf '%s\n' 'the previous build does not run either; not swapping' >&2; exit 1; }
/bin/mv "$previous" "$installed"
"$installed" --version >/dev/null 2>&1 \
  || { printf '%s\n' 'restored build does not run' >&2; exit 1; }

# The manifest described the build that was just removed. Left in place it
# would name a commit for bytes no longer on this host, which is worse than
# naming nothing: `host provenance` reports a missing manifest as
# unprovenanced, and unprovenanced is the truth here until the next install.
/bin/rm -f "$HOME/.stado/provenance/$name.json"
"#;

/// Put the previous build of one owner-only Stado program back on TARGET.
///
/// `install-binary` verifies that a new build runs, which is not the same as
/// verifying that the unit around it still works: a program can answer
/// `--version` perfectly and still reject the arguments its launchd job passes.
/// That failure appears after the swap, so the previous build is kept beside
/// the new one and this is how it comes back.
async fn rollback_binary(target: &str, name: &str, json: bool) -> Result<(), CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let quoted = crate::deploy::shlex_quote(name);
    let script = format!("set -euo pipefail\nname={quoted}\n{ROLLBACK_BODY}");
    let output = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target}: rollback failed: {}",
            crate::deploy::host_channel::last_error_line(&output, "remote rollback failed")
        )));
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "name": name,
                "provenance": crate::provenance::UNPROVENANCED,
                "status": "rolled-back",
            }))?
        );
    } else {
        println!(
            "{target}: {name} restored from the previous build; its provenance manifest was \
             dropped, because it described the build just removed"
        );
    }
    Ok(())
}

/// The far side of a streamed delivery: verify, then let the file take its
/// name.
///
/// `@SUBDIR@` and `@MODE@` are the only things that vary between a credential
/// and any other delivered file, and the mode is written symbolically so the
/// contract reads as what it grants rather than as a number to decode.
const STREAM_FILE_BODY: &str = r#"dir="$HOME/@SUBDIR@"
staged="$dir/.$name.stado-stream"
trap 'rm -f "$staged"' EXIT
[ -s "$staged" ] || { printf '%s\n' 'delivered file is missing or empty' >&2; exit 1; }
/bin/chmod @MODE@ "$staged"
line=$(/usr/bin/openssl dgst -sha256 -r "$staged")
actual="${line%% *}"
if [ "$actual" != "$expected" ]; then
  printf '%s\n' 'transfer checksum mismatch' > /dev/stderr
  exit 1
fi
/bin/mv "$staged" "$dir/$name"
trap - EXIT
printf '%s\n' "$dir/$name"
"#;

/// Deliver one file too large to embed in a script, and verify it landed.
///
/// Same contract as the inline path -- owner-only, checksummed on the far side
/// before it takes the name -- with the bytes carried by the transport instead
/// of the command line. `subdir` and `mode` are what separate a credential
/// from any other delivered file; everything else about the delivery is
/// identical, which is why there is one of these rather than two.
async fn stream_file(
    target: &str,
    source: &str,
    name: &str,
    subdir: &str,
    mode: &str,
) -> Result<(String, usize), CmdError> {
    release_component("delivered file name", name)?;
    let bytes = std::fs::metadata(source)?.len();
    let mut digest = Sha256::new();
    digest.update(std::fs::read(source)?);
    let expected_sha256 = hex::encode(digest.finalize());

    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let staged = format!("{subdir}/.{name}.stado-stream");

    let quoted_subdir = crate::deploy::shlex_quote(subdir);
    let prepare = crate::deploy::host_channel::run_script(
        &resolved,
        &format!(
            "set -euo pipefail\n/bin/mkdir -p \"$HOME\"/{quoted_subdir}\n\
             /bin/chmod u=rwx,go= \"$HOME\"/{quoted_subdir}\n"
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !prepare.ok() {
        return Err(CmdError::click(format!(
            "{target}: cannot prepare the delivery directory: {}",
            crate::deploy::host_channel::last_error_line(&prepare, "remote mkdir failed")
        )));
    }

    if crate::deploy::host_channel::target_is_this_host(&resolved) {
        let home = std::env::var("HOME")
            .map_err(|_| CmdError::click("HOME is not set, so the secret path is unknown"))?;
        std::fs::copy(source, std::path::Path::new(&home).join(&staged))?;
    } else {
        let ssh_target = resolved.ssh.clone().unwrap_or_default();
        if ssh_target.is_empty() {
            return Err(CmdError::click(format!(
                "{target} declares no ssh destination, so the file cannot be delivered"
            )));
        }
        let mut options = crate::deploy::host_channel::ssh_options(&ssh_target);
        options.pop();
        let mut argv = vec!["scp".to_string(), "-q".to_string()];
        argv.extend(options.into_iter().skip(usize::from(true)));
        argv.push(source.to_string());
        argv.push(format!("{ssh_target}:{staged}"));
        let copy = runner(crate::deploy::CommandSpec::new(argv))
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        if !copy.ok() {
            return Err(CmdError::click(format!(
                "{target}: cannot deliver the file: {}",
                copy.detail()
            )));
        }
    }

    let quoted_name = crate::deploy::shlex_quote(name);
    let quoted_sha = crate::deploy::shlex_quote(&expected_sha256);
    let script = format!(
        "set -euo pipefail\nname={quoted_name}\nexpected={quoted_sha}\n{}",
        STREAM_FILE_BODY
            .replace("@SUBDIR@", subdir)
            .replace("@MODE@", mode)
    );
    let output = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target}: delivery failed: {}",
            crate::deploy::host_channel::last_error_line(&output, "remote secret write failed")
        )));
    }
    Ok((
        format!("$HOME/{subdir}/{name}"),
        usize::try_from(bytes).unwrap_or(usize::MAX),
    ))
}

/// Read back both halves of the question: what the host runs, and what it can
/// account for.
///
/// Two enumerations rather than one. Listing only the manifests would answer
/// "what has been recorded", which is never the failing case -- a recorded
/// artifact is by definition one somebody bothered to record. The binaries in
/// `.stado/bin` are the population; the manifests are the coverage; the
/// difference is the finding.
///
/// `.previous` builds are skipped: they are the rollback copy of an artifact
/// already listed under its own name, and reporting a second unprovenanced row
/// for each installed program would bury the real ones.
///
/// Manifests are flattened to one line each so the two kinds of output can be
/// told apart by tag rather than by parsing position, and a host that answers
/// with unexpected noise cannot turn into a fabricated row.
const READ_PROVENANCE_BODY: &str = r#"bin="$HOME/.stado/bin"
dir="$HOME/.stado/provenance"
if [ -d "$bin" ]; then
  for program in "$bin"/*; do
    [ -f "$program" ] || continue
    case "${program##*/}" in .*|*.previous) continue ;; esac
    # A release artifact is a compiled program; a helper is a checked-in script
    # delivered by `host install-helper`. Both live in this directory and only
    # the first is something a release pipeline produces, so reporting them in
    # one list buries the question being asked. control-host carries dozens
    # of helpers accumulated over months -- helpers have a writer and no reaper,
    # the same accretion that fills ~/.stado/forwards with markers for services
    # that were renamed years of incidents ago. The shebang is the honest
    # discriminator and it is readable without executing anything.
    kind=binary
    case "$(/usr/bin/head -c 2 "$program" 2>/dev/null)" in '#!') kind=script ;; esac
    printf 'STADO-ARTIFACT %s %s\n' "$kind" "${program##*/}"
  done
fi
if [ -d "$dir" ]; then
  for manifest in "$dir"/*.json; do
    [ -f "$manifest" ] || continue
    printf 'STADO-MANIFEST %s\n' "$(/usr/bin/tr -d '\n\r' < "$manifest")"
  done
fi
"#;

/// One artifact a host carries, joined to whatever accounts for it.
struct CarriedArtifact {
    artifact: String,
    record: Option<crate::provenance::Provenance>,
    /// `None` is "no checkout here could answer", never "no". An operator who
    /// is told `no` walks a build back; one who is told `unknown` clones the
    /// repository first. Collapsing the two is how a fleet learns to disregard
    /// its own reports.
    reachable: Option<bool>,
    age_seconds: Option<i64>,
}

/// `stado host provenance TARGET [--json]` — what TARGET carries, and who
/// produced it.
///
/// The command that did not exist on 2026-08-11, when the only record of what
/// was running the control plane was a version string the repository had never
/// heard of. Every artifact under the host's Stado bin directory gets a row,
/// whether or not anything accounts for it, and an artifact with no manifest
/// is reported `unprovenanced` -- absent from the table is the one outcome
/// this must never produce, because that is precisely what the fleet did for
/// months.
///
/// Reachability is resolved here rather than read from the manifest, against a
/// checkout this process can see, because it is a question whose answer
/// changes: a commit unreachable at install time becomes reachable the moment
/// someone pushes, and a stored verdict would still be accusing them.
pub async fn provenance(target: &str, json: bool) -> Result<(), CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let script = format!("set -euo pipefail\n{READ_PROVENANCE_BODY}");
    let output = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target}: cannot read provenance manifests: {}",
            crate::deploy::host_channel::last_error_line(&output, "remote provenance read failed")
        )));
    }

    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut records: std::collections::BTreeMap<String, crate::provenance::Provenance> =
        std::collections::BTreeMap::new();
    let mut unreadable: Vec<String> = Vec::new();
    let mut helpers: usize = 0;
    for line in output.stdout.lines() {
        if let Some(artifact) = line.strip_prefix("STADO-ARTIFACT ") {
            // `<kind> <name>`. A helper script has no release behind it, so
            // listing it beside the control-plane binary answers a question
            // nobody asked and hides the one that matters.
            let mut words = artifact.trim().splitn(2, ' ');
            let kind = words.next().unwrap_or_default();
            let Some(name) = words.next().map(str::trim).filter(|name| !name.is_empty()) else {
                continue;
            };
            if kind == "script" {
                helpers += 1;
                continue;
            }
            names.insert(name.to_string());
        } else if let Some(document) = line.strip_prefix("STADO-MANIFEST ") {
            match serde_json::from_str::<crate::provenance::Provenance>(document.trim()) {
                Ok(record) => {
                    names.insert(record.artifact.clone());
                    records.insert(record.artifact.clone(), record);
                }
                // A manifest that cannot be parsed is not a missing manifest:
                // something wrote a file there and it says nothing usable.
                Err(error) => unreadable.push(error.to_string()),
            }
        }
    }

    let repository = crate::provenance::local_repo();
    let now = chrono::Utc::now();
    let carried: Vec<CarriedArtifact> = names
        .into_iter()
        .map(|artifact| {
            let record = records.remove(&artifact);
            let reachable = match (&record, &repository) {
                (None, _) => Some(false),
                (Some(record), _) if !record.names_a_commit() => Some(false),
                (Some(_), None) => None,
                (Some(record), Some(repository)) => Some(crate::provenance::reachable_in_repo(
                    &record.commit,
                    repository,
                )),
            };
            let age_seconds = record.as_ref().and_then(|record| {
                chrono::DateTime::parse_from_rfc3339(&record.at)
                    .ok()
                    .map(|stamp| (now - stamp.with_timezone(&chrono::Utc)).num_seconds())
            });
            CarriedArtifact {
                artifact,
                record,
                reachable,
                age_seconds,
            }
        })
        .collect();

    let commit_of = |item: &CarriedArtifact| {
        item.record.as_ref().map_or_else(
            || crate::provenance::UNPROVENANCED.to_string(),
            |record| record.commit.clone(),
        )
    };
    let drifted = carried
        .iter()
        .filter(|item| item.reachable != Some(true))
        .count();

    if json {
        let artifacts: Vec<Value> = carried
            .iter()
            .map(|item| {
                json!({
                    "artifact": item.artifact,
                    "manifest": item.record.is_some(),
                    "commit": commit_of(item),
                    "sha256": item.record.as_ref().map(|record| record.sha256.clone()),
                    "builder": item.record.as_ref().map(|record| record.builder.clone()),
                    "at": item.record.as_ref().map(|record| record.at.clone()),
                    "age_seconds": item.age_seconds,
                    "reachable": item.reachable,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "repository": repository.as_ref().map(|path| path.display().to_string()),
                "artifacts": artifacts,
                "unreadable_manifests": unreadable,
                "drifted": drifted,
            }))?
        );
        return Ok(());
    }

    let rows: Vec<Vec<String>> = carried
        .iter()
        .map(|item| {
            let age = match (item.age_seconds, &item.record) {
                (Some(seconds), _) => super::registry::human_age(chrono::TimeDelta::seconds(seconds)),
                // A manifest whose timestamp will not parse is a manifest
                // somebody hand-edited; say so instead of showing an age.
                (None, Some(_)) => "unknown".to_string(),
                (None, None) => "never".to_string(),
            };
            let reachable = match item.reachable {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            };
            vec![
                item.artifact.clone(),
                commit_of(item),
                item.record
                    .as_ref()
                    .map_or_else(|| "-".to_string(), |record| record.builder.clone()),
                age,
                reachable.to_string(),
            ]
        })
        .collect();
    // Before the early return as well as before the table: a host whose only
    // provenance file is corrupt must not read as a host with nothing to say.
    for error in &unreadable {
        eprintln!("{target}: a provenance manifest could not be read: {error}");
    }
    if rows.is_empty() {
        println!("{target}: carries no stado-managed programs");
        return Ok(());
    }
    super::table::print(
        &["ARTIFACT", "COMMIT", "BUILDER", "AGE", "REACHABLE"],
        &rows,
    );
    if repository.is_none() {
        println!(
            "\n{target}: no local checkout was found, so reachability is unknown rather than \
             answered; run this from the stado source tree to resolve it"
        );
    }
    if drifted != usize::default() {
        println!(
            "{target}: {drifted} of {} artifacts have no producer reachable from origin/main",
            rows.len()
        );
    }
    if helpers != usize::default() {
        // Not drift, and not nothing. Helpers are delivered one at a time to
        // solve one incident and are never removed, so the population only
        // grows; naming the count is what makes an operator notice that a
        // directory of them accumulated while nobody decided to keep any.
        println!(
            "{target}: {helpers} installed helper script(s) alongside, which carry no release \
             and are not counted above"
        );
    }
    Ok(())
}
