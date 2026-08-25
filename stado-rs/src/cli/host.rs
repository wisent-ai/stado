//! `stado host ...` — Rust implementations of the complete `host` group:
//! health, recovery, user provisioning, and Weles recordings policy, plus
//! the read-only diagnostics of `docs/missing-commands.md` items two
//! through six (`uptime`, `ping`, `disk`, `cleanup --dry-run`, `exec`),
//! which have no Python original and live in `crate::deploy::host_*`.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::io::Read;

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
/// `stado host publish-beacon FILE [--print]` — publish a locally collected
/// health document through the dedicated, route-scoped Stado control API.
///
/// This command deliberately has no direct-storage mode and does not consult
/// provider credentials. Missing URL/token configuration, an insecure remote
/// URL, an over-broad token file, malformed JSON, and an inconsistent server
/// acknowledgement all fail closed.
///
/// The `link` block is collected HERE rather than by the collector scripts,
/// because it is the one part of a beacon that cannot be assembled with `df`
/// and `launchctl`: it reads the power log and the tailnet, and a host that
/// went silent has to publish that account of itself or the silence leaves no
/// trace at all (see [`crate::deploy::host_link`]). Collection never blocks
/// the publish — every probe is capped and degrades to a null.
///
/// It is injected only into a document about THIS host. The macOS collector
/// also relays beacons for hosts that cannot publish for themselves, and
/// stamping this machine's connectivity onto another machine's document would
/// invent the very evidence the block exists to provide.
///
/// `--print` writes the document that would be published and publishes
/// nothing, so the collection can be inspected on a host without a beacon
/// grant and without touching the fleet's store.
pub async fn publish_beacon(source: &str, print: bool) -> Result<(), CmdError> {
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
    let mut document: Value = serde_json::from_slice(&bytes)
        .map_err(|error| CmdError::click(format!("host beacon is not valid JSON: {error}")))?;
    let host = document
        .as_object()
        .and_then(|value| value.get("host"))
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("host beacon must be an object with a string host"))?
        .to_string();
    if !valid_beacon_host(&host) {
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

    if beacon_is_this_host(&host) {
        let link =
            crate::deploy::host_link::collect_link(&crate::deploy::production_runner()).await;
        if let Some(object) = document.as_object_mut() {
            object.insert("link".to_string(), serde_json::to_value(&link)?);
        }
    }
    // The merged document is what gets published, so the bytes on the wire
    // are the bytes just validated plus the block collected here.
    let bytes = serde_json::to_vec(&document)?;
    if print {
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
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
    endpoint.query_pairs_mut().append_pair("host", &host);

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
        && payload.get("host").and_then(Value::as_str) == Some(host.as_str())
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

/// Is this beacon document about the machine running this command?
///
/// The beacon slug is the leading hostname label, lowercased — exactly how
/// the collector scripts spell it (`hostname -s | tr '[:upper:]' '[:lower:]'`)
/// and how the readers resolve a target back to its beacon object. A host
/// whose name cannot be read at all matches nothing, which keeps the relay
/// path from being mistaken for a self-publish.
fn beacon_is_this_host(host: &str) -> bool {
    let local = crate::targets::normalize_hostname(&crate::providers::vast::system_hostname());
    let slug = local.split('.').next().unwrap_or_default();
    !slug.is_empty() && slug == host
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
        return Err(CmdError::click("STADO_HOST_HEALTH_API_TOKEN_FILE is empty"));
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
///
/// The canonical remote registry remains the default and fleet-survival
/// authority. `bundled_registry` is an explicit break-glass path for repairing
/// the storage or authorization outage that made that authority unreadable.
///
/// `host_recovery::STATUS_BLOCKED` reaches that same exit 1: a pass that ran
/// to its last line while leaving a managed unit unloaded is not a recovery,
/// and the `skipped` and `blockers` arrays of the printed document say which
/// unit and what to run. Exit 0 from this command means every managed unit is
/// loaded.
pub async fn recover(target: &str, bundled_registry: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let report = if bundled_registry {
        let registry = crate::targets::load_bundled_registry()
            .map_err(|exc| CmdError::click(exc.to_string()))?;
        crate::deploy::host_recovery::recover_host_with_registry(&registry, target, &runner).await
    } else {
        crate::deploy::host_recovery::recover_host(target, &runner).await
    }
    .map_err(|exc| CmdError::click(exc.to_string()))?;
    println!(
        "{}",
        crate::deploy::host_recovery::to_sorted_pretty(&report)
    );
    if report.get("status").and_then(Value::as_str) != Some(crate::deploy::host_recovery::STATUS_OK)
    {
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
    let registry = super::registry::read_registry().await?;
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
    force: bool,
) -> Result<(), CmdError> {
    let resolved = registry_target(target).await?;
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_build_caches::run_on_host(
        &resolved,
        root,
        min_age_days,
        apply,
        force,
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

/// Persist TARGET's NVIDIA board power cap in the canonical registry and apply
/// it immediately. The local agent keeps reconciling the declaration, including
/// after driver resets and host reboots.
pub async fn gpu_power_limit(target: &str, watts: u32, json: bool) -> Result<(), CmdError> {
    if watts == 0 {
        return Err(CmdError::usage("WATTS must be a positive integer"));
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
        .ok_or_else(|| CmdError::click(format!("target not in registry: {target}")))?
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry target must be an object"))?;
    entry.insert("gpu_power_limit_watts".to_string(), Value::from(watts));
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let payload = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let registry = crate::targets::load_registry_from_str(&payload)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let resolved = registry
        .lookup(target)
        .cloned()
        .ok_or_else(|| CmdError::click(format!("target not in registry: {target}")))?;
    let generation = store.compare_and_swap(&current.version, &payload).await?;

    let script = format!(
        r#"set -eu
nvidia_smi=$(command -v nvidia-smi)
if [ -z "$nvidia_smi" ]; then
  printf '%s\n' 'nvidia-smi is unavailable' >&2
  exit 1
fi
indices=$("$nvidia_smi" --query-gpu=index --format=csv,noheader,nounits)
if [ -z "$indices" ]; then
  printf '%s\n' 'nvidia-smi returned no GPUs' >&2
  exit 1
fi
for gpu in $indices; do
  "$nvidia_smi" --id="$gpu" --power-limit={watts} >/dev/null
done
"$nvidia_smi" \
  --query-gpu=index,power.limit,power.min_limit,power.max_limit \
  --format=csv,noheader,nounits
"#
    );
    let runner = crate::deploy::production_runner();
    let output = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target}: registry now requires {watts} W at generation {generation}, but immediate reconciliation failed: {}",
            crate::deploy::host_channel::last_error_line(
                &output,
                "remote nvidia-smi power-limit update failed"
            )
        )));
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "target": target,
                "gpu_power_limit_watts": watts,
                "registry_generation": generation,
                "driver": output.stdout.trim(),
                "status": "reconciled",
            }))?
        );
    } else {
        println!("{target}: gpu_power_limit_watts={watts} (generation {generation})");
        print!("{}", output.stdout);
    }
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
    // Said after the cleanup state, because it is the answer to the question
    // that state raises: the janitor ran, it freed what it could, and the disk
    // is still full. macOS publishes no size for a snapshot, so the count and
    // the host's own names are all there is to print — and printing "0 bytes"
    // for them would be the false reassurance this block exists to prevent.
    let snapshots = report.get("local_snapshots");
    let names: Vec<&str> = snapshots
        .and_then(|value| value.get("names"))
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if snapshots.and_then(|value| value.get("supported")) == Some(&Value::Bool(true))
        && !names.is_empty()
    {
        println!(
            "\nlocal APFS snapshots: {} — their blocks are inside USED above, no \
             stado command removes them, and macOS reports no size for them. \
             Thin them with tmutil if the space is needed:",
            names.len()
        );
        for name in names {
            println!("  {name}");
        }
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

/// `stado host gates HOST [--json]` — why this host is claiming nothing, in
/// one payload.
///
/// The exit status follows `claiming`, the way `host ping`'s follows its
/// combined verdict, so `stado host reclaim mini --apply --reason … && stado
/// host gates mini` is a usable sentence and a blocked host cannot be
/// mistaken for a healthy one by a script that only reads status codes.
///
/// The Mac mini sat at roughly 2 GiB free against a 55 GiB policy, its agent
/// published `disk_pressure_unresolved` every tick, it claimed nothing for
/// hours, every release build queued behind it — and no command in this CLI
/// said any of it. This is that sentence.
pub async fn gates(host: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let gates = crate::deploy::host_gates::read_host_gates(host, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let report = Value::Object(crate::deploy::host_gates::to_report(&gates));
    if json {
        print_json(&report);
        return claiming_outcome(&gates);
    }
    println!("host:     {}", gates.host);
    println!("claiming: {}", if gates.claiming { "yes" } else { "no" });
    if gates.claiming {
        println!("blockers: none");
    } else {
        // The agent's own words, unabridged: whatever is printed here has to
        // be greppable in the code that published it.
        println!("blockers: {}", gates.blockers.join(", "));
    }
    println!(
        "disk:     {} free, low watermark {}, target {}, policy {}",
        gigabytes(gates.free_gb),
        gigabytes(gates.low_watermark_gb.map(|gb| gb as f64)),
        gigabytes(gates.target_free_gb.map(|gb| gb as f64)),
        gates.policy_mode.as_deref().unwrap_or("none declared"),
    );
    match gates.published_at.as_deref() {
        Some(published) => println!(
            "capacity: {} free slot(s) of {} declared, published {} ({})",
            gates
                .free_slots
                .map_or_else(|| "-".to_string(), |slots| slots.to_string()),
            gates.slots_declared,
            gates.age_seconds.map_or_else(
                || "at an unknown time".to_string(),
                |age| format!(
                    "{} ago",
                    super::registry::human_age(chrono::TimeDelta::seconds(age))
                )
            ),
            published,
        ),
        None => println!(
            "capacity: nothing published for this host, so the scheduler cannot \
             see it at all ({} slot(s) declared)",
            gates.slots_declared
        ),
    }
    // The consequence beside the cause: what this host's refusal is starving,
    // oldest first, so "blocked" has a size and an age.
    if !gates.waiting_jobs.is_empty() {
        println!(
            "waiting:  {} pinned job(s) this host is not taking: {}",
            gates.waiting_jobs.len(),
            gates
                .waiting_jobs
                .iter()
                .map(|job| {
                    let id = &job.job_id[..8.min(job.job_id.len())];
                    match job.age_seconds {
                        Some(age) => format!(
                            "{id} ({} in queue)",
                            super::registry::human_age(chrono::TimeDelta::seconds(age))
                        ),
                        None => id.to_string(),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // Printed after the verdict and never as part of it: a note is a thing the
    // operator has to know before they conclude the numbers do not add up, and
    // `stado host reclaim` is about to tell them it freed less than the deficit.
    for note in &gates.notes {
        if note == crate::deploy::host_gates::LOCAL_SNAPSHOTS_UNRECLAIMABLE {
            println!(
                "note:     {note} — {} local APFS snapshot(s) hold space no stado \
                 command reclaims, and macOS reports no size for them. See stado \
                 host disk {} for their names",
                gates
                    .local_snapshots
                    .map_or_else(|| "-".to_string(), |count| count.to_string()),
                gates.host,
            );
            continue;
        }
        println!("note:     {note}");
    }
    claiming_outcome(&gates)
}

/// GiB with one decimal, or a dash for a number this host did not answer with.
fn gigabytes(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_string(), |gb| format!("{gb} GiB"))
}

/// A host that is not claiming is a failed verdict, not a failed command: the
/// read succeeded either way, and the message names the blockers rather than
/// repeating that something is wrong.
fn claiming_outcome(gates: &crate::deploy::host_gates::HostGates) -> Result<(), CmdError> {
    if gates.claiming {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{} is claiming nothing: {}",
        gates.host,
        gates.blockers.join(", ")
    )))
}

/// How many silence records `stado host link` carries in its document.
///
/// Five, newest first: enough that a host which has been dropping off every
/// afternoon shows a pattern rather than a single incident, and few enough that
/// the document stays readable on a terminal during the outage it describes.
/// The full history stays in the store under `host_silence/<host>/`.
const NEWEST_SILENCES: usize = 5;

/// How far back `stado host link` counts what readers refused.
///
/// One hour rather than the silence threshold. The refusals a gap produces land
/// AROUND it, not inside it: on 2026-08-19 the resolver refused twice while the
/// beacon was still inside its tolerance, so a window as narrow as the
/// threshold would report the gap with none of the refusals it caused. An hour
/// is the span an operator asking "why did this host go quiet" has in mind, and
/// every refusal record keeps its own timestamp for any question longer than
/// that.
const REFUSAL_WINDOW_SECONDS: i64 = 60 * 60;

/// The beacon is fresh and nothing refused.
const LINK_HEALTHY: &str = "healthy";
/// Nothing has been heard from this host since the silence threshold.
const LINK_SILENT: &str = "silent";
/// Readers refused inside the window, or the host answers ssh while its own
/// beacon is stale.
const LINK_DEGRADED: &str = "degraded";

/// What a host publishes no path for. Never a fabricated `direct`: "we do not
/// know how this host is reachable" is the answer that sends an operator to
/// look, and a guess is the answer that does not.
const PATH_KIND_UNKNOWN: &str = "unknown";

/// `stado host link TARGET [--json]` — why this host went quiet, in one
/// payload.
///
/// The incident: between 18:29 and 18:35 UTC on 2026-08-19 control-host
/// answered no ping and no ssh, then came back on `direct 10.0.0.253:41641`.
/// Six minutes of a host being unreachable left no trace anywhere in this
/// product. The only evidence was two ping packets an operator happened to
/// send, and the reader-side refusals it caused — "service directory cache is
/// stale", "registry authority exited: ssh connect Operation timed out" — went
/// to `~/.stado/logs/stado-resolver.err` and nowhere a person would look. This
/// command is the trace: the host's own account of its path and its sleep and
/// wake times, the silences recorded against it, and what refused because of
/// them.
///
/// Everything here is read. Opening and closing a silence belongs to the
/// observer path in [`crate::monitor::host_silence`]; a diagnostic that
/// recorded a silence every time an operator looked would make the count it
/// prints a function of how often it was run.
///
/// The exit status follows `verdict`, the way `host gates`' follows `claiming`,
/// so `stado host link mini && ...` is a usable sentence and a silent host
/// cannot be mistaken for a healthy one by a script that reads only status
/// codes.
pub async fn link(target: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let store = beacon_store().await?;
    let mut blockers: Vec<String> = Vec::new();

    // The registry through the last-known-good cache, not the authority alone.
    // This is the command an operator runs while the control plane is the thing
    // that is sick: on 2026-08-19 every host command died on the same refused
    // ssh the operator was trying to diagnose, which is a diagnostic that dies
    // with its subject.
    let (registry, notice) = crate::targets::fetch_registry_or_last_good()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    if let Some(sentence) = notice {
        // On stderr so `--json` stays exactly one document on stdout, and in
        // the blockers so the cache's age reaches whoever reads the document
        // instead of the terminal.
        eprintln!("{sentence}");
        blockers.push(sentence);
    }
    let resolved = crate::deploy::host_channel::resolve_target(&registry, target)
        .map_err(|exc| CmdError::click(exc.to_string()))?;

    // The ssh half, through the same channel and the same fixed program
    // `host ping` sends, so the two commands can never disagree about whether
    // a host answers. A refused connection is this command's answer, not its
    // failure: "does not answer ssh" is precisely what was asked.
    let ssh = crate::deploy::host_channel::run_program(
        resolved,
        crate::deploy::host_ping::REMOTE_PROGRAM,
        &runner,
    )
    .await;
    let (ssh_reachable, ssh_error) = match &ssh {
        Ok(output) if output.ok() => (true, None),
        Ok(output) => (
            false,
            Some(crate::deploy::host_channel::last_error_line(
                output,
                "ssh failed",
            )),
        ),
        Err(exc) => (false, Some(exc.to_string())),
    };

    // The one fact neither surface could state, and the reason the mini takes
    // no work: whether anybody is logged in on its screen. Asked only of a
    // host that just answered, so an unreachable box costs one connect attempt
    // here rather than two, and answered by the same resolver
    // `stado service restart` uses, so a diagnostic and a repair cannot
    // disagree about the session underneath them.
    let session = match &ssh_error {
        None => crate::deploy::service::read_session(resolved, &runner).await,
        Some(detail) => crate::deploy::service::HostSession::unknown(format!(
            "this host did not answer, so nobody could ask it whether anyone is logged in on its \
             screen: {detail}"
        )),
    };

    // The beacon half, aged by the one rule `host ping` ages every beacon in
    // this fleet with, and the `link` block the host published inside it.
    let now = chrono::Utc::now();
    let (signal, published) =
        match crate::monitor::host_health::load_host_health(&store, &resolved.name).await {
            Ok(report) => {
                let published = crate::deploy::host_link::BeaconLink::from_beacon(&report.beacon)
                    .map(serde_json::to_value)
                    .transpose()?;
                (
                    crate::deploy::host_ping::grade_beacon(&report, now),
                    published,
                )
            }
            Err(exc) => (
                crate::deploy::host_ping::BeaconSignal::unreadable(exc.to_string()),
                None,
            ),
        };
    let from_link = |key: &str| {
        published
            .as_ref()
            .and_then(|block| block.get(key).cloned())
            .unwrap_or(Value::Null)
    };

    let threshold = crate::monitor::host_silence::silence_threshold_seconds();
    // No age at all — no beacon object, an unparseable one, an unreadable store
    // — counts as past the threshold. An absent beacon is the strongest form of
    // "nothing has been heard from this host", not an exemption from it.
    let stale = signal.age_seconds.is_none_or(|age| age > threshold);

    if let Some(detail) = &signal.error {
        blockers.push(detail.clone());
    }
    if let (true, Some(age)) = (stale, signal.age_seconds) {
        blockers.push(format!(
            "this host's newest beacon is {age}s old, past the {threshold}s silence threshold"
        ));
    }
    if let Some(detail) = &ssh_error {
        blockers.push(detail.clone());
    }
    if published.is_none() {
        blockers.push(
            "this host's beacon carries no link block, so its path, its sleep and wake \
             times and its interface changes are unknown here"
                .to_string(),
        );
    }

    // A headless host is not a fault, and the verdict rules do not learn about
    // this one. A headless host carrying a unit that only a logged-in screen
    // can start IS the fault, and it is the fault that stops work:
    // control-host has three of them and a job that has waited days for
    // the capacity they would publish. The declaration half is
    // `deploy::service::misdeclared_domains` rather than a second opinion
    // about it; what is added here is the half that had to be read from the
    // host. One blocker per unit, because each needs its own command run.
    if session.is_headless() {
        for misdeclared in crate::deploy::service::misdeclared_domains(resolved) {
            blockers.push(format!(
                "nobody is logged in on the screen here, and {} is registered as a user service, \
                 so this machine cannot start it; install it as a machine service with one \
                 privileged command on the host: {}",
                misdeclared.unit,
                misdeclared.install_command()
            ));
        }
    }

    // Looking at a beacon IS the observation the silence record is made of,
    // and [`crate::monitor::host_silence::observe_beacon_age`] is the one
    // entry point for the transition: whichever component notices the
    // threshold crossing writes it, and three observers of one gap produce one
    // record carrying three names. An operator running this command during an
    // outage is exactly that — the observer who noticed — and on 2026-08-19
    // nothing recorded what they saw. The instant is the beacon's own, recovered
    // with the same parser that aged it: a silence's `started_at` is when the
    // host was last heard from, and deriving it from the rounded age would
    // misdate every record by up to a second.
    let newest_beacon_at = signal
        .reported_at
        .as_deref()
        .and_then(crate::deploy::host_ping::parse_timestamp);
    if let Err(exc) = crate::monitor::host_silence::observe_beacon_age(
        &store,
        &resolved.name,
        newest_beacon_at,
        crate::monitor::host_silence::READER_CLI,
        signal.error.as_deref(),
    )
    .await
    {
        blockers.push(exc.to_string());
    }

    // A store that will not answer for the silences is reported as a blocker
    // and never as a failed command. Refusing to print the half that was read
    // is the exact behaviour this command exists to end.
    let silences = match crate::monitor::host_silence::recent_silences(
        &store,
        &resolved.name,
        NEWEST_SILENCES,
    )
    .await
    {
        Ok(records) => records,
        Err(exc) => {
            blockers.push(exc.to_string());
            Vec::new()
        }
    };
    let refusals = match crate::monitor::host_silence::refusal_summary(
        &store,
        &resolved.name,
        REFUSAL_WINDOW_SECONDS,
    )
    .await
    {
        Ok(summary) => summary,
        Err(exc) => {
            blockers.push(exc.to_string());
            crate::monitor::host_silence::RefusalSummary::empty(REFUSAL_WINDOW_SECONDS)
        }
    };
    let refused = refusals.count > usize::MIN;
    if refused {
        blockers.push(format!(
            "readers refused {} time(s) in the last {}s: {}",
            refusals.count,
            refusals.window_seconds,
            reason_counts(&refusals),
        ));
    }
    // The open record's own first reader error, verbatim: it is what a reader
    // wrote down at the moment the host stopped answering, and once the host is
    // back it is the only account of the gap that exists.
    if let Some(open) = silences.iter().find(|record| record.ended_at.is_none()) {
        let mut sentence = format!(
            "a silence opened at {} is still open",
            silence_instant(open.started_at)
        );
        if let Some(detail) = &open.first_reader_error {
            sentence.push_str(&format!("; first reader error: {detail}"));
        }
        blockers.push(sentence);
    }

    let verdict = if stale {
        // A box that answers ssh while nothing has heard from its agent is not
        // silent: it is running and not reporting, which is a different repair
        // and the exact state that ran for five days in July.
        if ssh_reachable {
            LINK_DEGRADED
        } else {
            LINK_SILENT
        }
    } else if refused {
        LINK_DEGRADED
    } else {
        LINK_HEALTHY
    };

    let path_kind = match from_link("path_kind") {
        Value::Null => Value::String(PATH_KIND_UNKNOWN.to_string()),
        kind => kind,
    };
    let changes = match from_link("interface_changes") {
        Value::Array(changes) => changes,
        _ => Vec::new(),
    };
    let recorded = silences
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<Value>, _>>()?;
    let report = json!({
        "host": resolved.name,
        "beacon_age_seconds": signal.age_seconds,
        "ssh_reachable": ssh_reachable,
        "session": session.to_json(),
        "path_kind": path_kind,
        "endpoint": from_link("endpoint"),
        "last_sleep_at": from_link("last_sleep_at"),
        "last_wake_at": from_link("last_wake_at"),
        "interface_changes": changes,
        "silences": recorded,
        "reader_refusals": {
            "window_seconds": refusals.window_seconds,
            "count": refusals.count,
            "reasons": refusals.reasons,
        },
        "verdict": verdict,
        "blockers": blockers,
    });
    if json {
        print_json(&report);
        return link_outcome(&resolved.name, verdict, blockers.len());
    }

    // The same facts in the shape `host gates` prints, so an operator reading
    // one of these two commands can read the other without relearning it.
    println!("host:     {}", resolved.name);
    println!("verdict:  {verdict}");
    if blockers.is_empty() {
        println!("blockers: none");
    } else {
        // One per line, unabridged. These are whole sentences from the reader,
        // the channel and the host's own agent; comma-joining them made three
        // accounts read as one.
        for (index, blocker) in blockers.iter().enumerate() {
            let label = if index == usize::MIN {
                "blockers:"
            } else {
                "         "
            };
            println!("{label} {blocker}");
        }
    }
    match (signal.age_seconds, signal.reported_at.as_deref()) {
        (Some(age), Some(reported)) => println!(
            "beacon:   {} old, reported {reported}",
            super::registry::human_age(chrono::TimeDelta::seconds(age))
        ),
        _ => println!("beacon:   nothing readable for this host"),
    }
    println!(
        "ssh:      {}",
        match &ssh_error {
            None => "answered".to_string(),
            Some(detail) => format!("did not answer: {detail}"),
        }
    );
    // The headline in the operator's words first, the resolver's own sentence
    // under it. Reversing those two is how `gui/501` becomes the answer to
    // "is anyone logged in on that host".
    println!("session:  {}", session.headline());
    println!("          {}", session.detail);
    // "unknown" alone, not "unknown via -": a host that published no endpoint
    // has one fact to report, and a dash standing in for a second one reads as
    // a field that failed rather than a field that does not apply.
    println!(
        "path:     {}",
        match published.as_ref().and_then(|block| block.get("endpoint")) {
            Some(Value::String(endpoint)) => format!("{} via {endpoint}", cell(Some(&path_kind))),
            _ => cell(Some(&path_kind)),
        }
    );
    println!(
        "sleep:    last slept {}, last woke {}",
        cell(
            published
                .as_ref()
                .and_then(|block| block.get("last_sleep_at"))
        ),
        cell(
            published
                .as_ref()
                .and_then(|block| block.get("last_wake_at"))
        ),
    );
    if changes.is_empty() {
        println!("changes:  none recorded");
    } else {
        println!("changes:  {} recorded", changes.len());
        for change in &changes {
            println!(
                "          {} {}",
                cell(change.get("at")),
                cell(change.get("detail"))
            );
        }
    }
    if refused {
        println!(
            "refusals: {} in the last {}s: {}",
            refusals.count,
            refusals.window_seconds,
            reason_counts(&refusals)
        );
    } else {
        println!("refusals: none in the last {}s", refusals.window_seconds);
    }
    if silences.is_empty() {
        println!("silences: none recorded for this host");
    } else {
        println!("silences: {} recorded, newest first", silences.len());
        for record in &silences {
            println!(
                "          {} -> {} ({}){}",
                silence_instant(record.started_at),
                record
                    .ended_at
                    .map_or_else(|| "still open".to_string(), silence_instant),
                record
                    .duration_seconds
                    .map_or_else(|| "-".to_string(), |seconds| format!("{seconds}s")),
                record
                    .first_reader_error
                    .as_deref()
                    .map_or_else(String::new, |detail| format!(
                        ", first reader error: {detail}"
                    )),
            );
        }
    }
    link_outcome(&resolved.name, verdict, blockers.len())
}

/// One silence instant, spelled the way the record on disk spells it.
///
/// `AutoSi` and a `Z`, which is what `chrono`'s own serialization writes into
/// the blob: the report is a pointer into `host_silence/<host>/`, and an
/// operator who copies the instant out of this report has to be able to find
/// the record it names. `to_rfc3339`'s `+00:00` would not match it.
fn silence_instant(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

/// `token=count` pairs for one refusal summary, in the stable order the
/// summary's own map holds them.
fn reason_counts(refusals: &crate::monitor::host_silence::RefusalSummary) -> String {
    refusals
        .reasons
        .iter()
        .map(|(reason, count)| format!("{reason}={count}"))
        .collect::<Vec<String>>()
        .join(", ")
}

/// A host whose link is not healthy is a failed verdict, not a failed command:
/// the read succeeded either way.
///
/// The blockers stay in the report and deliberately out of this sentence. They
/// carry the reader's and the channel's own words — "ssh connect Operation
/// timed out" among them — and [`crate::failure::classify_message`] reads
/// "timed out" in a command's failure message as a retryable failure, which
/// would remap this command's exit status away from the 1 that every
/// non-healthy verdict owes its caller.
fn link_outcome(host: &str, verdict: &str, blockers: usize) -> Result<(), CmdError> {
    if verdict == LINK_HEALTHY {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{host} link verdict is {verdict}, with {blockers} blocker(s) named in the report above"
    )))
}

/// `stado host reclaim HOST [--dry-run|--apply --reason TEXT] [--json]` — get
/// the space back, in declared stages, measuring each one.
///
/// Previewing is the default and `--apply` is the only thing that deletes,
/// because the alternative — a flag that has to be remembered to make the
/// command safe — is a flag that will be forgotten on the one host where it
/// mattered. `--apply` additionally refuses to run without `--reason`: the
/// record it appends on the host is the only account of why several tens of
/// gigabytes left that machine, and a record whose reason is blank is a record
/// nobody can act on six months later.
pub async fn reclaim(
    host: &str,
    apply: bool,
    reason: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let reason = reason.map(str::trim).filter(|text| !text.is_empty());
    if apply && reason.is_none() {
        return Err(CmdError::usage(
            "host reclaim --apply removes files and needs --reason <text>; the reason is \
             appended to the host's own audit log beside the disk it changed. Run without \
             --apply to see what each stage would remove",
        ));
    }
    let runner = crate::deploy::production_runner();
    let (target, reclamation) = crate::deploy::host_reclaim::reclaim_host(host, apply, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let audited = match reason {
        Some(reason) if apply => Some(
            crate::deploy::host_reclaim::record_audit(
                &target,
                &reclamation,
                reason,
                &super::autonomy_cmd::actor(),
                &runner,
            )
            .await
            .map_err(|exc| CmdError::click(exc.to_string()))?,
        ),
        _ => None,
    };
    let report = Value::Object(crate::deploy::host_reclaim::to_report(
        &target,
        &reclamation,
    ));
    if json {
        print_json(&report);
        return Ok(());
    }
    if apply {
        println!("APPLIED — {} lost the files named below.", target.name);
    } else {
        // Said before the table, not after it: an operator reading a list of
        // paths has to know which of the two things they are looking at.
        println!(
            "DRY RUN — nothing on {} is deleted. Re-run with --apply --reason <text> \
             to remove what follows.",
            target.name
        );
    }
    let rows: Vec<Vec<String>> = reclamation
        .stages
        .iter()
        .map(|stage| {
            vec![
                stage.stage.clone(),
                gigabytes(stage.free_kb_before.map(gib)),
                gigabytes(stage.free_kb_after.map(gib)),
                stage.items.to_string(),
            ]
        })
        .collect();
    super::table::print(&["STAGE", "FREE BEFORE", "FREE AFTER", "ITEMS"], &rows);
    for stage in &reclamation.stages {
        if let Some(detail) = &stage.detail {
            println!("{}: {detail}", stage.stage);
        }
        for path in &stage.paths {
            println!("  {} {path}", stage.stage);
        }
    }
    if let Some(plan) = &reclamation.janitor_plan {
        let cleaners: Vec<Vec<String>> = crate::deploy::host_cleanup::cleaner_plans(plan)
            .iter()
            .map(|cleaner| {
                vec![
                    cleaner.name.clone(),
                    cleaner.scanned_items.to_string(),
                    cleaner.eligible_items.to_string(),
                    cleaner.deleted_items.to_string(),
                ]
            })
            .collect();
        if !cleaners.is_empty() {
            println!("\nthe host's own janitor, per declared cleaner:");
            super::table::print(&["CLEANER", "SCANNED", "ELIGIBLE", "DELETED"], &cleaners);
        }
    }
    println!(
        "\nfree: {} -> {}",
        gigabytes(reclamation.free_kb_before.map(gib)),
        gigabytes(reclamation.free_kb_after.map(gib)),
    );
    if let Some(audited) = audited {
        println!("audited: {audited} on {}", target.name);
    }
    Ok(())
}

/// `df -Pk` blocks as GiB, through the one conversion `host disk` owns.
fn gib(blocks: i64) -> f64 {
    crate::deploy::host_disk::gib_from_blocks(blocks as f64)
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
            let registry = super::registry::read_registry().await?;
            registry
                .targets
                .iter()
                .map(|entry| entry.name.clone())
                .collect()
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
        let list = host
            .get("vaults")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        println!("{name}: {} vault(s)", list.len());
        for vault in list {
            println!(
                "  {:>5} items  {} recipients  {}",
                vault
                    .get("items")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                vault
                    .get("recipients")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                vault.get("path").and_then(Value::as_str).unwrap_or("")
            );
        }
    }
    println!(
        "{} host(s), {} unreachable, {} vault(s), {} item(s)",
        summary
            .get("hosts")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        summary
            .get("unreachable")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        summary
            .get("vaults")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        summary
            .get("items")
            .and_then(Value::as_u64)
            .unwrap_or_default()
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
    let binary = crate::deploy::products::product(binary)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let version = version.trim();
    if version.is_empty() {
        return Err(CmdError::usage("--version must name an exact version"));
    }
    let (mut document, expected_generation) = super::registry::fetch_versioned_document().await?;
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
    let generation = super::registry::push_document_if(&document, &expected_generation).await?;
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

/// Add or remove one action in a target's canonical Weles declaration.
///
/// The worker reads a generated placement-policy file, not the registry
/// directly. Keeping this command declaration-only preserves that boundary:
/// `publish-placement-policy` remains the single path that stamps and delivers
/// the registry generation to the host.
pub async fn declare_weles_action(
    target: &str,
    action: &str,
    remove: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let action = action.trim();
    if action.is_empty()
        || action == "*"
        || !action
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(CmdError::usage(
            "ACTION must contain only lowercase letters, digits and underscores",
        ));
    }

    let (mut document, expected_generation) = super::registry::fetch_versioned_document().await?;
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
    let weles = entry
        .get_mut("weles")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{target} declares no weles policy; refusing to invent one while adding an action"
            ))
        })?;
    let enabled = weles
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| CmdError::click(format!("{target}.weles.enabled must be a boolean")))?;
    let values = weles
        .get("actions")
        .and_then(Value::as_array)
        .ok_or_else(|| CmdError::click(format!("{target}.weles.actions must be an array")))?;
    let mut actions = values
        .iter()
        .map(|value| {
            value.as_str().map(str::to_string).ok_or_else(|| {
                CmdError::click(format!(
                    "{target}.weles.actions contains a non-string value"
                ))
            })
        })
        .collect::<Result<Vec<String>, CmdError>>()?;

    let changed = if remove {
        let before = actions.len();
        actions.retain(|declared| declared != action);
        before != actions.len()
    } else if actions.iter().any(|declared| declared == action) {
        false
    } else {
        actions.push(action.to_string());
        true
    };
    if enabled && actions.is_empty() {
        return Err(CmdError::click(format!(
            "removing {action:?} would leave enabled {target}.weles.actions empty"
        )));
    }
    actions.sort();
    actions.dedup();

    let generation = if changed {
        weles.insert("actions".to_string(), json!(actions));
        super::registry::push_document_if(&document, &expected_generation).await?
    } else {
        expected_generation
    };
    let state = if changed {
        if remove {
            "removed"
        } else {
            "added"
        }
    } else {
        "unchanged"
    };

    if json_output {
        print_json(&json!({
            "target": target,
            "action": action,
            "state": state,
            "generation": generation,
            "publish_command": format!("stado host publish-placement-policy {target}"),
        }));
        return Ok(());
    }
    println!(
        "{target}: Weles action {action} {state} in registry generation {generation}; \
         publish with `stado host publish-placement-policy {target}`"
    );
    Ok(())
}

/// Promote one published version into fleet desired state in one fenced
/// registry write. Every platform manifest must already exist and identify
/// the canonical coordinate before `managed_versions` moves.
pub async fn promote_version(
    binary: &str,
    version: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let managed = crate::deploy::products::product(binary)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let version = version.trim();
    if !crate::deploy::host_release::is_exact_semver(version) {
        return Err(CmdError::usage(
            "--version must name an exact immutable semantic version",
        ));
    }
    crate::cli::storage::release_api_origin()?;
    let (mut document, expected_generation) = super::registry::fetch_versioned_document().await?;
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
        let observed = crate::deploy::products::managed_platform(observed)
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
        // A product publishes for the platforms it declares, and promoting a
        // version onto a fleet includes hosts it may not publish for at all.
        // Refused rather than skipped: a declaration a host can never receive
        // is drift this pack has no way to close.
        managed
            .platform(platform)
            .map_err(|error| CmdError::click(error.to_string()))?;
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
            CmdError::click(format!(
                "target {name:?} was not inventoried before promotion"
            ))
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
    let generation = super::registry::push_document_if(&document, &expected_generation).await?;
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
    let registry = super::registry::read_registry().await?;
    let names: Vec<String> = match target {
        Some(name) => {
            if registry.targets.iter().all(|entry| entry.name != name) {
                return Err(CmdError::click(format!(
                    "registry declares no target {name:?}"
                )));
            }
            vec![name]
        }
        None => registry
            .targets
            .iter()
            .map(|entry| entry.name.clone())
            .collect(),
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

    let healthy = standings
        .iter()
        .all(crate::deploy::reconcile::HostStanding::settled)
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
                    standing.target, standing.declared_release_platform, standing.release_platform
                );
            }
            if standing.settled() {
                println!("{}: active versions match desired state", standing.target);
            }
            for drift in &standing.drift {
                println!(
                    "{}: {} is {} — desired {}, active {}",
                    standing.target, drift.binary, drift.verdict, drift.declared, drift.installed
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
                delivery
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
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
    reinstall: bool,
    json: bool,
) -> Result<(), CmdError> {
    use crate::deploy::host_release;

    let runner = crate::deploy::production_runner();
    let report = host_release::release_host(target, binary, version, dry_run, reinstall, &runner)
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
    println!(
        "sha256:   {} (release manifest)",
        cell(report.get("sha256"))
    );
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

/// Where a delivered file lands, relative to the target account's home.
///
/// Separate from `.stado` itself so a delivery can never take the name of a
/// credential, a helper, or anything else Stado keeps there.
const DELIVERED_FILES_DIR: &str = ".stado/files";

pub(crate) async fn install_secret_value_at_home(
    target: &str,
    name: &str,
    value: &str,
    home: &str,
) -> Result<(String, usize), CmdError> {
    transfer_secret(target, name, value.as_bytes(), Some(home)).await
}

/// Deliver one file through the [`stream_file`] channel and RETURN where it
/// landed, for a caller that renders its own report.
///
/// A callee that prints is unusable from a machine-readable caller:
/// `stado host publish-placement-policy --json` would put a delivery report in
/// front of its own document and hand the operator two JSON objects on one
/// stream. Same channel, same checksum, same owner-only mode — only the
/// reporting belongs to whoever asked.
pub(crate) async fn deliver_file(
    target: &str,
    source: &str,
    name: &str,
) -> Result<(String, usize), CmdError> {
    stream_file(target, source, name, DELIVERED_FILES_DIR, "u=rw,go=").await
}

/// The registration `stado host sync-acquisition-scopes` performs on the host,
/// natively: the checks and key steps of the retired registration script as
/// individual remote commands, with every branch taken here. Modeled on
/// weles's register-weles-acquisition-scopes-host.sh with the two appstore
/// token re-mints removed — minting weles worker credentials is not part of
/// registering a catalog, and every re-mint silently extended those tokens'
/// expiry.
///
/// Everything about the registration is fixed: the vault, the workload key,
/// and the single skarbiec call. The one operator-chosen word — the delivered
/// catalog's basename — was validated by [`catalog_file_name`] before
/// delivery and is validated again below, so the file this reads is decided
/// here, not by whoever wrote the variable.
///
/// The return is the one line the retired script printed, composed here.
/// Failures divide the way the channel always divided them: a transport error
/// is returned as-is, and a remote refusal is wrapped with the delivered path
/// so the operator can tell "delivered and not registered" from "never
/// reached the host".
async fn register_acquisition_scopes(
    resolved: &ComputeTarget,
    delivered: &str,
    catalog_name: &str,
    runner: &crate::deploy::Runner,
) -> Result<String, CmdError> {
    use crate::deploy::host_channel;

    // A remote refusal: the script's own words, wrapped with which half of
    // the operation happened.
    let refused = |detail: String| {
        CmdError::click(format!(
            "{}: the catalog reached {delivered} and was NOT registered: {detail}. \
             Settle the refusal and sync again",
            resolved.name
        ))
    };

    if catalog_name.is_empty()
        || catalog_name.starts_with('.')
        || !catalog_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(refused("invalid catalog file name".to_string()));
    }

    let home = host_channel::remote_home(resolved, runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let bin = format!("{home}/.stado/bin/skarbiec");
    let vault = format!("{home}/.stado/skarbiec.vault.json");
    let private_key = format!("{home}/.stado/weles-credential-workload-private.pem");
    let catalog = format!("{home}/.stado/files/{catalog_name}");

    for file in [&bin, &vault, &private_key, &catalog] {
        let present = host_channel::remote_test(
            resolved,
            &format!("-f {}", crate::deploy::shlex_quote(file)),
            runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        if !present {
            return Err(refused(format!(
                "required acquisition-scope file is missing: {file}"
            )));
        }
    }

    let brewed = "/opt/homebrew/opt/openssl@3/bin/openssl";
    let openssl = if host_channel::remote_test(resolved, &format!("-x {brewed}"), runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        brewed.to_string()
    } else {
        let looked_up = host_channel::run_command(resolved, "command -v openssl", runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let found = looked_up.stdout.trim();
        if found.is_empty() {
            return Err(refused(
                "openssl is required to derive the workload public key".to_string(),
            ));
        }
        found.to_string()
    };

    let public_key =
        match acquisition_scratch(resolved, &home, "weles-acquisition-public.XXXXXX", runner).await
        {
            Ok(path) => path,
            Err(detail) => return Err(refused(detail)),
        };

    // Skarbiec accepts only an Ed25519 workload key. A host still holding an
    // older key gets one Ed25519 replacement, and the new private key takes
    // the canonical path only after registration with its public half
    // succeeded.
    let mut candidate_key = private_key.clone();
    let mut new_private_key: Option<String> = None;
    let described = host_channel::run_program(
        resolved,
        &[
            openssl.as_str(),
            "pkey",
            "-in",
            private_key.as_str(),
            "-text",
            "-noout",
        ],
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !described.stdout.contains("ED25519") {
        let fresh =
            match acquisition_scratch(resolved, &home, "weles-acquisition-private.XXXXXX", runner)
                .await
            {
                Ok(path) => path,
                Err(detail) => {
                    remove_remote(resolved, &[public_key.as_str()], runner).await;
                    return Err(refused(detail));
                }
            };
        for words in [
            vec![
                openssl.as_str(),
                "genpkey",
                "-algorithm",
                "ED25519",
                "-out",
                fresh.as_str(),
            ],
            vec!["/bin/chmod", "600", fresh.as_str()],
        ] {
            let stepped = host_channel::run_program(resolved, &words, runner)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
            if !stepped.ok() {
                remove_remote(resolved, &[public_key.as_str(), fresh.as_str()], runner).await;
                return Err(refused(host_channel::last_error_line(
                    &stepped,
                    "openssl could not generate an Ed25519 workload key",
                )));
            }
        }
        candidate_key = fresh.clone();
        new_private_key = Some(fresh);
    }

    let derived = host_channel::run_program(
        resolved,
        &[
            openssl.as_str(),
            "pkey",
            "-in",
            candidate_key.as_str(),
            "-pubout",
            "-out",
            public_key.as_str(),
        ],
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !derived.ok() {
        let mut litter = vec![public_key.as_str()];
        if let Some(fresh) = &new_private_key {
            litter.push(fresh.as_str());
        }
        remove_remote(resolved, &litter, runner).await;
        return Err(refused(host_channel::last_error_line(
            &derived,
            "openssl could not derive the workload public key",
        )));
    }

    let registered = host_channel::run_command(
        resolved,
        &format!(
            "SKARBIEC_VAULT_FILE={} {} token-register-acquisitions {} \
             --workload-public-key-file {} --replace-capabilities >/dev/null",
            crate::deploy::shlex_quote(&vault),
            crate::deploy::shlex_quote(&bin),
            crate::deploy::shlex_quote(&catalog),
            crate::deploy::shlex_quote(&public_key),
        ),
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !registered.ok() {
        let mut litter = vec![public_key.as_str()];
        if let Some(fresh) = &new_private_key {
            litter.push(fresh.as_str());
        }
        remove_remote(resolved, &litter, runner).await;
        return Err(refused(host_channel::last_error_line(
            &registered,
            "remote registration failed",
        )));
    }

    if let Some(fresh) = &new_private_key {
        let moved = host_channel::run_program(
            resolved,
            &["/bin/mv", "-f", fresh.as_str(), private_key.as_str()],
            runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        if !moved.ok() {
            remove_remote(resolved, &[public_key.as_str(), fresh.as_str()], runner).await;
            return Err(refused(host_channel::last_error_line(
                &moved,
                "the new Ed25519 workload key could not be moved into place",
            )));
        }
    }
    remove_remote(resolved, &[public_key.as_str()], runner).await;

    Ok(format!(
        "{{\"status\":\"reconciled\",\"catalog\":\"{catalog_name}\"}}\n"
    ))
}

/// One scratch file in the host's own `.stado` directory, owner-only from the
/// moment `mktemp` creates it.
async fn acquisition_scratch(
    resolved: &ComputeTarget,
    home: &str,
    suffix: &str,
    runner: &crate::deploy::Runner,
) -> Result<String, String> {
    let made = crate::deploy::host_channel::run_command(
        resolved,
        &format!(
            "mktemp {}",
            crate::deploy::shlex_quote(&format!("{home}/.stado/{suffix}"))
        ),
        runner,
    )
    .await
    .map_err(|error| error.to_string())?;
    if !made.ok() {
        return Err(crate::deploy::host_channel::last_error_line(
            &made,
            "could not create a scratch file on the host",
        ));
    }
    Ok(made.stdout.trim().to_string())
}

/// Best-effort removal of this registration's scratch files — the retired
/// script's EXIT trap. A failure to remove is not a failure of the
/// registration that already happened, so it is ignored here exactly as the
/// trap's `rm -f` ignored it there.
async fn remove_remote(resolved: &ComputeTarget, paths: &[&str], runner: &crate::deploy::Runner) {
    let mut words = vec!["/bin/rm", "-f"];
    words.extend_from_slice(paths);
    let _ = crate::deploy::host_channel::run_program(resolved, &words, runner).await;
}

/// The basename a local catalog is delivered and registered under.
///
/// A name, never a path: it becomes one component under
/// `$HOME/.stado/files` on the host, so it follows the delivered-name rules
/// ([`release_component`]) and additionally may not start with `.` — no
/// hidden files, and no `.`/`..` components, whichever spelling produced
/// them.
fn catalog_file_name(source: &str) -> Result<String, CmdError> {
    let name = std::path::Path::new(source)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CmdError::usage("catalog path must end in a file name"))?;
    release_component("catalog file name", name)?;
    if name.starts_with('.') {
        return Err(CmdError::usage("catalog file name must not start with '.'"));
    }
    Ok(name.to_string())
}

/// `stado host sync-acquisition-scopes TARGET SOURCE` — deliver the checked-in
/// Skarbiec acquisition-scope catalog to TARGET and register it against the
/// host's fleet vault.
///
/// Two audited halves and no third way in: the catalog travels through the
/// [`stream_file`] delivery channel into `$HOME/.stado/files`, owner-only
/// and checksummed on arrival, and the registration is
/// [`register_acquisition_scopes`] — there is nothing to install on the host
/// and nothing left behind but the delivered catalog. This is the reviewed
/// replacement for running weles's register script through the retired helper
/// channel.
pub async fn sync_acquisition_scopes(target: &str, source: &str) -> Result<(), CmdError> {
    let metadata = std::fs::symlink_metadata(source)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CmdError::usage("catalog source must be a regular file"));
    }
    let name = catalog_file_name(source)?;
    let (delivered, _bytes) = deliver_file(target, source, &name).await?;

    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let printed = register_acquisition_scopes(&resolved, &delivered, &name, &runner).await?;
    print!("{printed}");
    if !printed.ends_with('\n') {
        println!();
    }
    Ok(())
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

/// Remove one file from TARGET's home: the path Stado never had a way to
/// delete, so a retired or broken unit left its plist on disk forever and the
/// only answer was a bare `rm` over ssh, which nothing bounds and nobody
/// audits. This is that answer as a product verb. The guards are on the host,
/// not on the client, because the file is what the host says it is, not what
/// the operator believes:
///
/// - the path must be absolute, contain no `..`, and live under
///   `$HOME/Library/LaunchAgents` or `$HOME/.stado` of the approved account —
///   a system path is not refused because it is dangerous, it is refused
///   because this channel has no right there, and the refusal names the
///   privileged command that does have one;
/// - it must be a regular file owned by that account — a symlink under an
///   allowed root can point anywhere, a directory would make this a recursive
///   delete, and somebody else's file is not this login's to remove.
///
/// Absence is reported as `absent`, not invented into a success.
/// The outcome of one [`remove_file_document`] call, so a composed command
/// (`service remove`) can carry the file half as data instead of scraping
/// another command's stdout.
pub struct RemoveFileOutcome {
    pub target: String,
    pub path: String,
    pub status: String,
    pub detail: Option<String>,
}

impl RemoveFileOutcome {
    pub fn succeeded(&self) -> bool {
        self.status == "removed" || self.status == "absent"
    }

    fn failure_sentence(&self) -> String {
        format!(
            "{}: {} {}{}",
            self.target,
            self.path,
            self.status,
            self.detail
                .as_ref()
                .map(|detail| format!(" — {detail}"))
                .unwrap_or_default()
        )
    }
}

/// The guarded delete itself, as a value: validation, resolution, the fixed
/// remote script, the marker read. Printing belongs to the caller.
pub async fn remove_file_document(target: &str, path: &str) -> Result<RemoveFileOutcome, CmdError> {
    if !path.starts_with('/') || path.contains("..") || path.contains('\0') {
        return Err(CmdError::usage(
            "path must be absolute, contain no '..', and carry no NUL",
        ));
    }
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let quoted = crate::deploy::shlex_quote(path);
    let script = format!(
        r#"set -u
path={quoted}
report() {{ printf 'STADO_REMOVE_FILE\t%s\t%s\n' "$1" "$2"; }}
case "$path" in
  "$HOME/Library/LaunchAgents/"*|"$HOME/.stado/"*) ;;
  *) report refused "outside the managed home areas; remove it on the host with: sudo rm -- $path"; exit 0 ;;
esac
if [ -L "$path" ]; then
  report refused "a symlink points outside the managed area; remove it by hand: rm -- $path"
elif [ -d "$path" ]; then
  report refused "a directory is not removed by a single-file command"
elif [ ! -e "$path" ]; then
  report absent ""
elif [ ! -O "$path" ]; then
  report refused "not owned by this account; remove it on the host with: sudo rm -- $path"
elif [ ! -f "$path" ]; then
  report refused "not a regular file"
else
  rm -f -- "$path"
  if [ -e "$path" ]; then
    report failed "rm succeeded and the path is still there"
  else
    report removed ""
  fi
fi
"#
    );
    let output = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        &script,
        std::time::Duration::from_secs(60),
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    let (state, detail) = output
        .stdout
        .lines()
        .find_map(|line| {
            crate::deploy::host_channel::marker_fields(line)
                .as_slice()
                .split_first()
                .and_then(|(marker, rest)| {
                    (*marker == "STADO_REMOVE_FILE")
                        .then(|| (rest[0].to_string(), rest.get(1).map(|s| s.to_string())))
                })
        })
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: the host answered without a removal report: {}",
                resolved.name,
                crate::deploy::host_channel::last_error_line(&output, "no marker in output")
            ))
        })?;
    let outcome = RemoveFileOutcome {
        target: resolved.name.clone(),
        path: path.to_string(),
        status: state,
        detail,
    };
    if outcome.succeeded() {
        Ok(outcome)
    } else {
        Err(CmdError::click(outcome.failure_sentence()))
    }
}

/// Remove one file from TARGET's home: the path Stado never had a way to
/// delete, so a retired or broken unit left its plist on disk forever and the
/// only answer was a bare `rm` over ssh, which nothing bounds and nobody
/// audits. This is that answer as a product verb. The guards are on the host,
/// not on the client, because the file is what the host says it is, not what
/// the operator believes:
///
/// - the path must be absolute, contain no `..`, and live under
///   `$HOME/Library/LaunchAgents` or `$HOME/.stado` of the approved account —
///   a system path is not refused because it is dangerous, it is refused
///   because this channel has no right there, and the refusal names the
///   privileged command that does have one;
/// - it must be a regular file owned by that account — a symlink under an
///   allowed root can point anywhere, a directory would make this a recursive
///   delete, and somebody else's file is not this login's to remove.
///
/// Absence is reported as `absent`, not invented into a success.
pub async fn remove_file(target: &str, path: &str, json: bool) -> Result<(), CmdError> {
    let outcome = remove_file_document(target, path).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": outcome.target,
                "path": outcome.path,
                "status": outcome.status,
                "detail": outcome.detail,
            }))?
        );
    } else {
        match &outcome.detail {
            Some(detail) if !detail.is_empty() => {
                println!(
                    "{}: {} {} — {detail}",
                    outcome.target, outcome.path, outcome.status
                )
            }
            _ => println!("{}: {} {}", outcome.target, outcome.path, outcome.status),
        }
    }
    Ok(())
}

/// A vault item id or tag: the alphabet `release_component` allows, plus the
/// `:` that every one of these names is built out of
/// (`provider:kimi:brama-sub-…`, `brama:agent:wisent-app`).
///
/// Checked here because these words are interpolated into a script that
/// performs an owner write, and a name that arrived from an inventory is no
/// more trustworthy than one an operator typed.
fn vault_word(kind: &str, value: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err(CmdError::usage(format!(
            "{kind} must contain only letters, digits, '.', '_', '-' or ':'"
        )));
    }
    Ok(())
}

/// What the host reported for one phase of the retag.
struct RetagPhase {
    state: String,
    revision: String,
    tags: String,
}

/// One item of the host's vault, read as a retag phase: its state, revision
/// and tags, or `absent` when the vault holds no such item. The vault is read
/// over the channel and parsed here — the phase rendering the retired
/// script's python snippet produced, without a python payload.
async fn read_vault_phase(
    resolved: &ComputeTarget,
    vault: &str,
    item: &str,
    runner: &crate::deploy::Runner,
) -> Result<RetagPhase, String> {
    let text = crate::deploy::host_channel::remote_read_file(resolved, vault, runner)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("the vault at {vault} could not be read"))?;
    let document: Value = serde_json::from_str(&text)
        .map_err(|error| format!("the vault at {vault} did not parse as JSON: {error}"))?;
    let Some(record) = document.get("items").and_then(|items| items.get(item)) else {
        return Ok(RetagPhase {
            state: "absent".to_string(),
            revision: "-".to_string(),
            tags: "-".to_string(),
        });
    };
    let state = record
        .get("state")
        .and_then(Value::as_str)
        .filter(|state| !state.is_empty())
        .unwrap_or("-")
        .to_string();
    let revision = match record.get("revision") {
        Some(Value::String(revision)) => revision.clone(),
        Some(Value::Number(revision)) => revision.to_string(),
        _ => "-".to_string(),
    };
    let tags = record
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<&str>>()
                .join(",")
        })
        .filter(|tags| !tags.is_empty())
        .unwrap_or_else(|| "-".to_string());
    Ok(RetagPhase {
        state,
        revision,
        tags,
    })
}

/// Replace one Skarbiec item's tags on TARGET, and report what the host had
/// before and has after.
///
/// Tags decide who may spend a credential: Brama treats a vault item as a
/// subscription only when it carries `brama:subscription` and
/// `brama:agent:<agent>`, so an item that loses them leaves the fleet while
/// remaining perfectly valid — silently, because a credential nobody can see
/// still passes every check that counts credentials. Restoring them is a write
/// only the owner key can make, and that key lives on the host, so this runs
/// there and nowhere else.
///
/// Tags only: the payload is never read, rewritten or re-encrypted, which is
/// the whole reason this is not a `set-json`.
pub async fn retag_vault_item(
    target: &str,
    item: &str,
    tags: &str,
    json: bool,
) -> Result<(), CmdError> {
    vault_word("vault item", item)?;
    for tag in tags.split(',') {
        vault_word("tag", tag)?;
    }
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    // The host's own overrides, resolved on the host the way the retired
    // script's `${VAR:-default}` did.
    let environment = crate::deploy::host_channel::run_command(
        &resolved,
        "printf '%s\\n%s\\n' \"${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}\" \
         \"${GNUPGHOME:-$HOME/.gnupg}\"",
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !environment.ok() {
        return Err(CmdError::click(format!(
            "{}: {item} could not be retagged: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(
                &environment,
                "the host's vault environment could not be read"
            )
        )));
    }
    let mut variables = environment.stdout.lines();
    let vault = variables.next().unwrap_or_default().to_string();
    let gnupg_home = variables.next().unwrap_or_default().to_string();
    let skarbiec = format!("{home}/.stado/bin/skarbiec");

    // A remote refusal names the check that failed, in the words the retired
    // script printed to stderr.
    let refused = |detail: String| {
        CmdError::click(format!(
            "{}: {item} could not be retagged: {detail}",
            resolved.name
        ))
    };
    if !crate::deploy::host_channel::remote_test(
        &resolved,
        &format!("-x {}", crate::deploy::shlex_quote(&skarbiec)),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?
    {
        return Err(refused(format!("no Skarbiec binary at {skarbiec}")));
    }
    if !crate::deploy::host_channel::remote_test(
        &resolved,
        &format!("-f {}", crate::deploy::shlex_quote(&vault)),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?
    {
        return Err(refused(format!("no vault at {vault}")));
    }
    // Whether this build can retag at all. The discriminator is the usage
    // literal, never the bare command name: rustc packs string literals into
    // one unterminated blob, so a binary that carries the command shows
    // `...setgetretagdelete...` on a single line and a whole-line match for
    // `retag` reports absent on a build that has it. That false negative cost
    // an hour and sent one diagnosis at the wrong host.
    let capable = crate::deploy::host_channel::run_command(
        &resolved,
        &format!(
            "strings -a {} 2>/dev/null | grep -q 'usage: retag <id> --tags'",
            crate::deploy::shlex_quote(&skarbiec)
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !capable.ok() {
        return Err(refused(format!(
            "the Skarbiec build at {skarbiec} predates the retag operation"
        )));
    }

    // The caller states what the host had and has rather than asserting
    // success: read the item before, retag, read it again.
    let before = read_vault_phase(&resolved, &vault, item, &runner)
        .await
        .map_err(refused)?;
    let retagged = crate::deploy::host_channel::run_command(
        &resolved,
        &format!(
            "GNUPGHOME={} SKARBIEC_VAULT_FILE={} {} retag {} --tags {} > /dev/null",
            crate::deploy::shlex_quote(&gnupg_home),
            crate::deploy::shlex_quote(&vault),
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(item),
            crate::deploy::shlex_quote(tags),
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !retagged.ok() {
        return Err(refused(crate::deploy::host_channel::last_error_line(
            &retagged,
            "remote retag failed",
        )));
    }
    let after = read_vault_phase(&resolved, &vault, item, &runner)
        .await
        .map_err(|detail| {
            CmdError::click(format!(
                "{}: {item} reported no tags after the retag; the host said: {detail}",
                resolved.name
            ))
        })?;
    let before = Some(before);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "item": item,
                "before": before.as_ref().map(|phase| json!({
                    "state": phase.state,
                    "revision": phase.revision,
                    "tags": phase.tags,
                })),
                "after": {
                    "state": after.state,
                    "revision": after.revision,
                    "tags": after.tags,
                },
            }))?
        );
    } else {
        if let Some(phase) = &before {
            println!(
                "{}: {item} had rev={} state={} tags={}",
                resolved.name, phase.revision, phase.state, phase.tags
            );
        }
        println!(
            "{}: {item} now rev={} state={} tags={}",
            resolved.name, after.revision, after.state, after.tags
        );
    }
    Ok(())
}

/// One declared log path out of a unit plist: `StandardOutPath` or
/// `StandardErrorPath`, or nothing when the plist does not declare it.
/// PlistBuddy writes its "does not exist" to stderr and prints nothing, so a
/// failed read is simply no path.
async fn unit_log_path(
    resolved: &ComputeTarget,
    key: &str,
    plist: &str,
    runner: &crate::deploy::Runner,
) -> Result<Option<String>, CmdError> {
    let output = crate::deploy::host_channel::run_program(
        resolved,
        &["/usr/libexec/PlistBuddy", "-c", key, plist],
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    let declared = output.stdout.trim();
    Ok((output.ok() && !declared.is_empty()).then(|| declared.to_string()))
}

/// The tail of one managed unit's own log, from the paths its unit file
/// declares.
///
/// A unit that crash-loops states why in its log and nowhere else: the health
/// beacon reports `failed` with an empty `last_log`, `service status` reports
/// the state, and `host exec` is a read-only allowlist that cannot read a file.
/// Without this the only route to the sentence naming the fault was an ssh
/// session, which is the one thing the fleet does not allow, so the fault got
/// guessed at instead.
pub async fn unit_log(
    target: &str,
    unit: &str,
    lines: Option<u32>,
    json: bool,
) -> Result<(), CmdError> {
    // A unit label is a reverse-DNS name; it names the plist files this reads,
    // so it is checked before it gets there.
    vault_word("unit label", unit)?;
    let lines = lines.unwrap_or(40).clamp(u32::from(true), 500);
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;

    // The unit file is found, never guessed: the daemon directory first, then
    // both agent directories, exactly the retired reader's search order.
    let mut plist = None;
    for candidate in [
        format!("/Library/LaunchDaemons/{unit}.plist"),
        format!("{home}/Library/LaunchAgents/{unit}.plist"),
        format!("/Library/LaunchAgents/{unit}.plist"),
    ] {
        if crate::deploy::host_channel::remote_test(
            &resolved,
            &format!("-f {}", crate::deploy::shlex_quote(&candidate)),
            &runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        {
            plist = Some(candidate);
            break;
        }
    }
    let Some(plist) = plist else {
        return Err(CmdError::click(format!(
            "{}: {unit} log could not be read: no unit file for {unit} in the daemon or agent \
             directories",
            resolved.name
        )));
    };

    // The report is composed here, in the wire text the retired reader
    // printed: STADO_UNITLOG marker lines interleaved with the prefixed
    // tails, so the JSON and text renderings below parse it unchanged.
    let mut report = format!("STADO_UNITLOG\tplist\t{plist}\n");

    // One reader for both keys: a unit that sends stdout and stderr to the
    // same file must not be tailed twice, and a unit that separates them must
    // not have half of its account silently dropped.
    let out_path = unit_log_path(&resolved, "Print :StandardOutPath", &plist, &runner).await?;
    let err_path = unit_log_path(&resolved, "Print :StandardErrorPath", &plist, &runner).await?;
    let mut declared: Vec<String> = Vec::new();
    if let Some(path) = &out_path {
        declared.push(path.clone());
    }
    if let Some(path) = &err_path {
        if out_path.as_ref() != Some(path) {
            declared.push(path.clone());
        }
    }
    if declared.is_empty() {
        return Err(CmdError::click(format!(
            "{}: {unit} log could not be read: {unit} declares no log path",
            resolved.name
        )));
    }

    for log in &declared {
        if crate::deploy::host_channel::remote_test(
            &resolved,
            &format!("-f {}", crate::deploy::shlex_quote(log)),
            &runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        {
            report.push_str(&format!("STADO_UNITLOG\tfile\t{log}\n"));
            report.push_str(&format!("=== {log} (last {lines} lines)\n"));
            let tail = crate::deploy::host_channel::run_program(
                &resolved,
                &["/usr/bin/tail", "-n", &lines.to_string(), "--", log],
                &runner,
            )
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
            if tail.ok() {
                report.push_str(&tail.stdout);
            } else {
                report.push_str("    unreadable\n");
            }
        } else {
            report.push_str(&format!("STADO_UNITLOG\tabsent\t{log}\n"));
            report.push_str(&format!("=== {log} (absent)\n"));
        }
    }

    let body: String = report
        .lines()
        .filter(|line| !line.starts_with("STADO_UNITLOG\t"))
        .collect::<Vec<_>>()
        .join("\n");
    if json {
        let files: Vec<serde_json::Value> = report
            .lines()
            .filter_map(
                |line| match crate::deploy::host_channel::marker_fields(line).as_slice() {
                    ["STADO_UNITLOG", kind, value] => Some(json!({
                        "kind": kind,
                        "path": value,
                    })),
                    _ => None,
                },
            )
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "unit": unit,
                "lines": lines,
                "declared": files,
                "log": body,
            }))?
        );
    } else {
        println!("{body}");
    }
    Ok(())
}

/// What one Weles worker host is doing: the Node.js program that reads the
/// worker's run evidence on the host itself and prints one JSON document.
///
/// Fed to the host's own `node` over the channel's stdin, with the run limit
/// and API port as argv — the same two values the retired bash wrapper took
/// from the host's environment. There is nothing to install on the host and
/// nothing left behind after the read.
///
/// Recordings hold page DOM, console output, HAR bodies, personas and proxy
/// identities. None of that is emitted. What leaves the host is counts,
/// timestamps, run identifiers, artifact sizes, cost, and the pass/fail flag a
/// trajectory wrote about itself — the fields a remote operator view needs to
/// name a run and say how it ended.
const WELES_ACTIVITY_SOURCE: &str = r#"const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');

const runLimit = Math.max(1, Number.parseInt(process.argv.at(-2), 10) || 40);
const apiPort = Number.parseInt(process.argv.at(-1), 10) || 8788;
const home = os.homedir();
const workerRoot = path.join(home, '.local/share/weles-worker');

const hostname = String(os.hostname()).trim().toLowerCase().replace(/\.+$/, '');
const shortHostname = hostname.endsWith('.local') ? hostname.slice(0, -'.local'.length) : hostname;

const isoOrNull = (value) => {
  const time = Number(value);
  return Number.isFinite(time) && time > 0 ? new Date(time).toISOString() : null;
};

const readJson = (file) => {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
};

// The version marker names the release the activator staged; the directories say
// which releases actually ran here. The two disagree while a deploy is mid
// flight, and a report that carried only one of them would hide that.
const releaseMarker = (() => {
  try {
    return fs.readFileSync(path.join(home, '.stado/files/weles-release-version'), 'utf8').trim() || null;
  } catch {
    return null;
  }
})();

const compareVersions = (left, right) => {
  const parts = (value) => String(value).split('.').map((piece) => Number.parseInt(piece, 10) || 0);
  const [a, b] = [parts(left), parts(right)];
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
};

let releases = [];
try {
  releases = fs
    .readdirSync(workerRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort(compareVersions);
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}

const ARTIFACT_CLASSES = [
  ['screenshots', /\.png$/i],
  ['pages', /\.html$/i],
  ['videos', /\.webm$/i],
  ['logs', /\.(log|ndjson)$/i],
  ['records', /\.json$|\.jsonl$|\.har$/i],
];

const classify = (name) => {
  for (const [label, pattern] of ARTIFACT_CLASSES) {
    if (pattern.test(name)) return label;
  }
  return 'other';
};

const RUNNING_WINDOW_MS = 180_000;

const describeRun = (release, platform, runDirectory) => {
  const stat = fs.statSync(runDirectory);
  const counts = { screenshots: 0, pages: 0, videos: 0, logs: 0, records: 0, other: 0 };
  let bytes = 0;
  let action = null;
  let resultOk = null;
  let startedAt = null;
  let completedAt = null;

  const walk = (directory, depth) => {
    let entries = [];
    try {
      entries = fs.readdirSync(directory, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const full = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        // The one directory directly under a run is the action that produced it.
        if (depth === 0 && !action) action = entry.name;
        if (depth < 4) walk(full, depth + 1);
        continue;
      }
      if (!entry.isFile()) continue;
      counts[classify(entry.name)] += 1;
      try {
        bytes += fs.statSync(full).size;
      } catch {
        // A file rotated away mid-walk is not worth failing the report over.
      }
      if (/result\.json$/i.test(entry.name)) {
        const document = readJson(full);
        if (document && typeof document.ok === 'boolean') resultOk = document.ok;
        if (typeof document?.completed_at === 'string') completedAt = document.completed_at;
      } else if (entry.name === 'session_meta.json') {
        const document = readJson(full);
        if (typeof document?.started_at === 'string') startedAt = document.started_at;
      }
    }
  };
  walk(runDirectory, 0);

  const uploaded = fs.existsSync(path.join(runDirectory, '.uploaded.json'));
  const costs = readJson(path.join(path.dirname(runDirectory), '_costs', `${path.basename(runDirectory)}.json`));
  const isFresh = Date.now() - stat.mtimeMs < RUNNING_WINDOW_MS;

  let status = 'recorded';
  if (resultOk === true) status = 'succeeded';
  else if (resultOk === false) status = 'failed';
  else if (isFresh) status = 'running';

  return {
    id: path.basename(runDirectory),
    release,
    platform,
    action,
    status,
    started_at: startedAt ?? isoOrNull(stat.birthtimeMs),
    completed_at: completedAt,
    updated_at: isoOrNull(stat.mtimeMs),
    artifact_counts: counts,
    artifact_bytes: bytes,
    cost_usd: typeof costs?.cost_usd === 'number' ? costs.cost_usd : null,
    uploaded,
  };
};

const runs = [];
let runTotal = 0;
for (const release of releases) {
  const releaseRoot = path.join(workerRoot, release);
  let platforms = [];
  try {
    platforms = fs
      .readdirSync(releaseRoot, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name);
  } catch {
    continue;
  }
  for (const platform of platforms) {
    const recordings = path.join(releaseRoot, platform, 'recordings');
    let entries = [];
    try {
      entries = fs.readdirSync(recordings, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      // `_costs` is the sidecar ledger of the runs beside it, not a run.
      if (!entry.isDirectory() || entry.name === '_costs') continue;
      runTotal += 1;
      runs.push({ release, platform, directory: path.join(recordings, entry.name) });
    }
  }
}

runs.sort((left, right) => {
  const time = (row) => {
    try {
      return fs.statSync(row.directory).mtimeMs;
    } catch {
      return 0;
    }
  };
  return time(right) - time(left);
});

const described = runs.slice(0, runLimit).map((row) => describeRun(row.release, row.platform, row.directory));

const probePort = (port) =>
  new Promise((resolve) => {
    const socket = net.createConnection({ host: '127.0.0.1', port });
    const finish = (listening) => {
      socket.destroy();
      resolve(listening);
    };
    socket.setTimeout(1500);
    socket.once('connect', () => finish(true));
    socket.once('timeout', () => finish(false));
    socket.once('error', () => finish(false));
  });

probePort(apiPort).then((listening) => {
  const document = {
    schema_version: 1,
    host: shortHostname || hostname,
    hostname,
    generated_at: new Date().toISOString(),
    worker: {
      staged_release: releaseMarker,
      installed_releases: releases,
      newest_release: releases.at(-1) ?? null,
    },
    api: {
      endpoint: `http://127.0.0.1:${apiPort}`,
      listening,
    },
    run_total: runTotal,
    runs: described,
  };
  process.stdout.write(`STADO-WELES-ACTIVITY ${JSON.stringify(document)}\n`);
});
"#;

/// The marker [`WELES_ACTIVITY_SOURCE`] prefixes to its one JSON line, so a
/// login shell's own greeting cannot be mistaken for the report.
const WELES_ACTIVITY_MARKER: &str = "STADO-WELES-ACTIVITY ";

/// Run [`WELES_ACTIVITY_SOURCE`] on one host with the host's own node, and
/// hand back what it printed.
///
/// The run limit and API port are the host's environment or the defaults the
/// retired wrapper carried, resolved on the host so an operator's local
/// environment cannot steer a remote read.
async fn read_weles_activity(
    resolved: &ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<String, crate::deploy::DeployError> {
    use crate::deploy::host_channel;
    let mut node = None;
    for candidate in ["/opt/homebrew/bin/node", "/usr/local/bin/node"] {
        if host_channel::remote_test(resolved, &format!("-x {candidate}"), runner).await? {
            node = Some(candidate);
            break;
        }
    }
    let Some(node) = node else {
        return Err(crate::deploy::DeployError(
            "Node.js is unavailable on this host".to_string(),
        ));
    };
    let environment = host_channel::run_command(
        resolved,
        "printf '%s %s' \"${WELES_ACTIVITY_RUN_LIMIT:-40}\" \"${WELES_API_PORT:-8788}\"",
        runner,
    )
    .await?;
    if !environment.ok() {
        return Err(crate::deploy::DeployError(host_channel::last_error_line(
            &environment,
            "the host's Weles environment could not be read",
        )));
    }
    let mut values = environment.stdout.split_whitespace();
    let limit = values.next().unwrap_or("40");
    let port = values.next().unwrap_or("8788");
    let output = host_channel::run_program_with_stdin(
        resolved,
        &[node, "-", limit, port],
        WELES_ACTIVITY_SOURCE,
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(crate::deploy::DeployError(host_channel::last_error_line(
            &output,
            "the Weles activity read did not complete",
        )));
    }
    Ok(output.stdout)
}

/// Report TARGET's Weles worker releases, API reachability and recorded runs.
pub async fn weles_activity(target: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| {
            CmdError::click(format!("{target}: cannot read Weles activity: {error}"))
        })?;
    let output = read_weles_activity(&resolved, &runner)
        .await
        .map_err(|error| {
            CmdError::click(format!("{target}: cannot read Weles activity: {error}"))
        })?;

    let document = output
        .lines()
        .filter_map(|line| line.trim().strip_prefix(WELES_ACTIVITY_MARKER))
        .next_back()
        .ok_or_else(|| {
            CmdError::click(format!(
                "{target}: the Weles activity read printed no report line"
            ))
        })?;
    let report: Value = serde_json::from_str(document).map_err(|error| {
        CmdError::click(format!(
            "{target}: the Weles activity report is not readable JSON: {error}"
        ))
    })?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let worker = &report["worker"];
    println!(
        "{target}: worker {} staged, {} newest installed, API {} on {}",
        worker["staged_release"].as_str().unwrap_or("unknown"),
        worker["newest_release"].as_str().unwrap_or("unknown"),
        if report["api"]["listening"].as_bool().unwrap_or_default() {
            "answering"
        } else {
            "silent"
        },
        report["api"]["endpoint"]
            .as_str()
            .unwrap_or("unknown endpoint"),
    );
    let runs = report["runs"].as_array().map_or(&[][..], Vec::as_slice);
    println!(
        "{target}: {} recorded run(s), {} newest below",
        report["run_total"].as_u64().unwrap_or_default(),
        runs.len()
    );
    // Newest first, exactly as the host ordered them: a run's own verdict and
    // the time it last wrote are what an operator reads to know where work is.
    for run in runs {
        println!(
            "  {:<10} {:<22} {:<38} {}",
            run["status"].as_str().unwrap_or("unknown"),
            run["action"].as_str().unwrap_or("unknown action"),
            run["id"].as_str().unwrap_or("-"),
            run["updated_at"].as_str().unwrap_or("-"),
        );
    }
    Ok(())
}

/// `stado host weles-capture TARGET --plan PLAN.json [--batch ID] [--json]` —
/// enqueue one batch of `generic_capture` actions on TARGET's Weles admission
/// API.
///
/// The plan is validated in full before the host is contacted: one bad capture
/// refuses the whole plan, because a half-enqueued batch still renders pages
/// and still writes artifacts, and nothing downstream can tell those from the
/// ones somebody planned.
pub async fn weles_capture(
    target: &str,
    plan: &str,
    batch: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let plan = crate::deploy::weles_capture::parse_plan(plan, target, batch)
        .map_err(|error| CmdError::usage(error.to_string()))?;
    let admission = crate::deploy::weles_capture::resolve_admission(target)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let channel = crate::deploy::weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let accepted = crate::deploy::weles_capture::enqueue(&channel, &plan)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    if json {
        print_json(&json!({
            "target": target,
            "batch": plan.batch,
            "action": crate::deploy::weles_capture::CAPTURE_ACTION,
            "endpoint": admission.declared_url,
            "transport": channel.transport(),
            "admission_token": channel.token_state(),
            "enqueued": accepted.len(),
            "actions": accepted
                .iter()
                .map(|action| json!({
                    "action_id": action.action_id,
                    "site_slug": action.site_slug,
                    "axis": action.axis,
                    "artifact_prefix": action.artifact_prefix,
                }))
                .collect::<Vec<Value>>(),
            "status": "enqueued",
        }));
        return Ok(());
    }
    println!(
        "{target}: enqueued {} {} action(s) for batch {} on {}",
        accepted.len(),
        crate::deploy::weles_capture::CAPTURE_ACTION,
        plan.batch,
        admission.declared_url,
    );
    for action in &accepted {
        println!(
            "  {:<38} {:<24} {:<13} {}",
            action.action_id, action.site_slug, action.axis, action.artifact_prefix,
        );
    }
    Ok(())
}

/// `stado host weles-capture-status TARGET --batch ID [--json]` — per-action
/// state of one capture batch, and the artifact keys already in Stado storage
/// under that batch's prefix. Read-only.
///
/// The exit status answers whether the batch is KNOWN, not whether every
/// capture succeeded: a runner polls this in a loop and a failed capture is a
/// row to read, not an error to retry. A batch nobody enqueued exits non-zero,
/// after printing the report, because that is the one question the report
/// cannot answer by being empty.
pub async fn weles_capture_status(target: &str, batch: &str, json: bool) -> Result<(), CmdError> {
    let admission = crate::deploy::weles_capture::resolve_admission(target)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let channel = crate::deploy::weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let batch_status = crate::deploy::weles_capture::status(&channel, batch)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let states = batch_status.captures;
    let totals = crate::deploy::weles_capture::totals(&states);
    let stored: usize = states.iter().map(|state| state.artifacts.len()).sum();
    if json {
        print_json(&json!({
            "target": target,
            "batch": batch,
            "action": crate::deploy::weles_capture::CAPTURE_ACTION,
            "endpoint": admission.declared_url,
            "transport": channel.transport(),
            "artifacts_unreachable": batch_status.artifacts_unreachable,
            "actions": states
                .iter()
                .map(|state| json!({
                    "action_id": state.action_id,
                    "site_slug": state.site_slug,
                    "axis": state.axis,
                    "state": state.state,
                    "error": state.error,
                    "artifact_prefix": state.artifact_prefix,
                    "artifacts": state.artifacts,
                }))
                .collect::<Vec<Value>>(),
            "totals": totals
                .iter()
                .map(|(state, count)| (state.clone(), Value::from(*count)))
                .collect::<serde_json::Map<String, Value>>(),
            "artifacts_stored": stored,
        }));
    } else {
        println!(
            "{target}: batch {batch} carries {} {} action(s), {}",
            states.len(),
            crate::deploy::weles_capture::CAPTURE_ACTION,
            totals
                .iter()
                .map(|(state, count)| format!("{count} {state}"))
                .collect::<Vec<String>>()
                .join(", "),
        );
        if let Some(unreachable) = &batch_status.artifacts_unreachable {
            println!("{target}: artifact listing unreadable: {unreachable}");
        }
        for state in &states {
            println!(
                "  {:<9} {:<24} {:<13} {:>3} artifact(s)  {}{}",
                state.state,
                state.site_slug,
                state.axis,
                state.artifacts.len(),
                state.action_id,
                state
                    .error
                    .as_deref()
                    .map_or_else(String::new, |error| format!("  {error}")),
            );
        }
        println!(
            "{target}: {stored} object(s) under stado://{}/{batch}/",
            crate::deploy::weles_capture::ARTIFACT_NAMESPACE
        );
    }
    if states.is_empty() {
        return Err(CmdError::click(format!(
            "{target}: no {} action carries batch {batch}, so nothing was ever enqueued under that id",
            crate::deploy::weles_capture::CAPTURE_ACTION
        )));
    }
    Ok(())
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

/// Open a background SSH forward from this host to TARGET's loopback.
/// Both ends bind loopback; SSH supplies transport encryption and refuses to
/// report success unless the local listener is established.
pub async fn forward_remote(
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
            "forward-remote requires a remote registry target",
        ));
    }
    let ssh = resolved
        .ssh
        .as_deref()
        .ok_or_else(|| CmdError::click("registry target has no SSH destination"))?;
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    let marker_directory = std::path::Path::new(&home).join(".stado").join("forwards");
    std::fs::create_dir_all(&marker_directory)?;
    let marker_path = marker_directory.join(format!("{name}.url"));
    let control_path = marker_directory.join(format!("{name}.control"));
    if marker_path.exists() || control_path.exists() {
        return Err(CmdError::click(format!(
            "{name}: forward state already exists; stop it with `stado host forward-stop {target} {name}`"
        )));
    }
    let endpoint = format!("http://127.0.0.1:{local_port}");
    std::fs::write(&marker_path, format!("{endpoint}\n"))?;
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
        "-o".to_string(),
        "ControlMaster=yes".to_string(),
        "-o".to_string(),
        format!("ControlPath={}", control_path.display()),
        "-o".to_string(),
        "ControlPersist=no".to_string(),
        "-L".to_string(),
        format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}"),
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
        let _ = std::fs::remove_file(&marker_path);
        let _ = std::fs::remove_file(&control_path);
        return Err(CmdError::click(format!(
            "{target}: SSH forwarding failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next_back()
                .unwrap_or("ssh forwarding failed")
        )));
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "remote": format!("127.0.0.1:{remote_port}"),
                "local": format!("127.0.0.1:{local_port}"),
                "marker": marker_path,
                "control": control_path,
                "transport": "ssh",
                "status": "forwarding",
            }))?
        );
    } else {
        println!(
            "{target}: forwarding local 127.0.0.1:{local_port} to 127.0.0.1:{remote_port} over SSH"
        );
    }
    Ok(())
}

/// Stop one named `forward-remote` channel.
///
/// Current forwards have an OpenSSH control socket and shut down through that
/// socket. Forwards created by older Stado versions have only the URL marker;
/// those are stopped only when the process listening on the marker's loopback
/// port is provably the matching SSH `-L` command to this registry target.
pub async fn forward_stop(target: &str, name: &str, json: bool) -> Result<(), CmdError> {
    release_component("forward name", name)?;
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if crate::deploy::host_channel::target_is_this_host(&resolved) {
        return Err(CmdError::usage(
            "forward-stop requires a remote registry target",
        ));
    }
    let ssh = resolved
        .ssh
        .as_deref()
        .ok_or_else(|| CmdError::click("registry target has no SSH destination"))?;
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    let marker_directory = std::path::Path::new(&home).join(".stado").join("forwards");
    let marker_path = marker_directory.join(format!("{name}.url"));
    let control_path = marker_directory.join(format!("{name}.control"));
    let endpoint = std::fs::read_to_string(&marker_path).ok();
    let local_port = endpoint
        .as_deref()
        .and_then(crate::deploy::host_inventory::marker_port);

    if !marker_path.exists() && !control_path.exists() {
        return Err(CmdError::click(format!(
            "{name}: no forward state exists in {}",
            marker_directory.display()
        )));
    }

    let mut stopped = false;
    let mut method = "control";
    if control_path.exists() {
        let mut argv = crate::deploy::host_channel::ssh_options(ssh);
        let destination = argv
            .pop()
            .ok_or_else(|| CmdError::click("SSH channel has no destination"))?;
        argv.extend([
            "-S".to_string(),
            control_path.display().to_string(),
            "-O".to_string(),
            "exit".to_string(),
            destination,
        ]);
        let (program, arguments) = argv
            .split_first()
            .ok_or_else(|| CmdError::click("SSH channel is empty"))?;
        stopped = tokio::process::Command::new(program)
            .args(arguments)
            .output()
            .await?
            .status
            .success();
    }

    if !stopped {
        method = "verified-legacy-process";
        if let Some(port) = local_port {
            let output = tokio::process::Command::new("/usr/sbin/lsof")
                .args(["-nP", "-t", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
                .output()
                .await?;
            let mut pids: Vec<i32> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().parse().ok())
                .collect();
            pids.sort_unstable();
            pids.dedup();
            if pids.len() > 1 {
                return Err(CmdError::click(format!(
                    "{name}: refusing to stop {port}; more than one process owns the listener"
                )));
            }
            if let Some(pid) = pids.first().copied() {
                let process = tokio::process::Command::new("/bin/ps")
                    .args(["-p", &pid.to_string(), "-o", "command="])
                    .output()
                    .await?;
                let command = String::from_utf8_lossy(&process.stdout);
                let executable = command
                    .split_whitespace()
                    .next()
                    .and_then(|value| std::path::Path::new(value).file_name())
                    .and_then(|value| value.to_str());
                let forward = format!("127.0.0.1:{port}:127.0.0.1:");
                if executable != Some("ssh")
                    || !command.contains(&forward)
                    || !command.contains(ssh)
                {
                    return Err(CmdError::click(format!(
                        "{name}: refusing to stop pid {pid}; it is not the matching SSH forward"
                    )));
                }
                nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGTERM,
                )
                .map_err(|error| {
                    CmdError::click(format!("{name}: could not stop forward pid {pid}: {error}"))
                })?;
                for _ in 0..20 {
                    if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() {
                        stopped = true;
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                if !stopped {
                    return Err(CmdError::click(format!(
                        "{name}: forward pid {pid} did not stop after SIGTERM"
                    )));
                }
            } else {
                method = "stale-state";
                stopped = true;
            }
        }
    }

    if !stopped {
        return Err(CmdError::click(format!(
            "{name}: no controllable SSH forward or verifiable legacy listener was found"
        )));
    }
    if marker_path.exists() {
        std::fs::remove_file(&marker_path)?;
    }
    if control_path.exists() {
        std::fs::remove_file(&control_path)?;
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "name": name,
                "local": local_port.map(|port| format!("127.0.0.1:{port}")),
                "method": method,
                "status": "stopped",
            }))?
        );
    } else {
        println!("{target}: stopped forward {name} ({method})");
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
    # left over from the retired helper channel. Both live in this directory and
    # only the first is something a release pipeline produces, so reporting them
    # in one list buries the question being asked. control-host carries
    # dozens of helpers accumulated over months -- the channel had a writer and
    # no reaper, the same accretion that fills ~/.stado/forwards with markers
    # for services that were renamed years of incidents ago. The shebang is the
    # honest discriminator and it is readable without executing anything.
    kind=binary
    case "$(/usr/bin/head -c 2 "$program" 2>/dev/null)" in '#!') kind=script ;; esac
    # The manifest is a claim about specific bytes. Reporting its commit without
    # checking it still describes the file beside it is the same unverified
    # declaration this command exists to find: on 2026-08-12 this laptop's
    # manifest named a commit while the binary next to it had been replaced by
    # hand, and the tool repeated the manifest with a straight face.
    digest=-
    if [ "$kind" = binary ]; then
      if [ -x /usr/bin/shasum ]; then
        digest=$(/usr/bin/shasum -a 256 "$program" | /usr/bin/awk '{print $1}')
      elif command -v sha256sum >/dev/null 2>&1; then
        digest=$(sha256sum "$program" | /usr/bin/awk '{print $1}')
      fi
    fi
    printf 'STADO-ARTIFACT %s %s %s\n' "$kind" "$digest" "${program##*/}"
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
    /// Does the manifest still describe the bytes beside it? `None` when there
    /// is no manifest or the host could not hash the file. A manifest naming a
    /// commit for a binary that has since been replaced is worse than no
    /// manifest: it answers the provenance question confidently and wrongly,
    /// which is exactly the failure this command was built to expose.
    describes: Option<bool>,
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
    let mut present: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for line in output.stdout.lines() {
        if let Some(artifact) = line.strip_prefix("STADO-ARTIFACT ") {
            // `<kind> <digest> <name>`. A helper script has no release behind
            // it, so listing it beside the control-plane binary answers a
            // question nobody asked and hides the one that matters.
            let mut words = artifact.trim().splitn(3, ' ');
            let kind = words.next().unwrap_or_default();
            let digest = words.next().unwrap_or_default().trim().to_string();
            let Some(name) = words.next().map(str::trim).filter(|name| !name.is_empty()) else {
                continue;
            };
            if kind == "script" {
                helpers += 1;
                continue;
            }
            if !digest.is_empty() && digest != "-" {
                present.insert(name.to_string(), digest);
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
            let describes = match (&record, present.get(&artifact)) {
                (Some(record), Some(actual)) => Some(record.sha256.eq_ignore_ascii_case(actual)),
                _ => None,
            };
            CarriedArtifact {
                artifact,
                record,
                reachable,
                describes,
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
                    "describes_artifact": item.describes,
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
                (Some(seconds), _) => {
                    super::registry::human_age(chrono::TimeDelta::seconds(seconds))
                }
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
            let describes = match item.describes {
                Some(true) => "match",
                Some(false) => "REPLACED",
                None => "-",
            };
            vec![
                item.artifact.clone(),
                commit_of(item),
                item.record
                    .as_ref()
                    .map_or_else(|| "-".to_string(), |record| record.builder.clone()),
                age,
                reachable.to_string(),
                describes.to_string(),
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
        &["ARTIFACT", "COMMIT", "BUILDER", "AGE", "REACHABLE", "BYTES"],
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
    let replaced = carried
        .iter()
        .filter(|item| item.describes == Some(false))
        .count();
    if replaced != usize::default() {
        // Louder than drift, because the manifest is not merely absent: it
        // answers the provenance question, and its answer is about bytes that
        // are gone. Every reader downstream inherits that wrong answer.
        println!(
            "{target}: {replaced} artifact(s) were replaced after their manifest was written, so \
             the commit shown for them describes bytes that are no longer on the host"
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

/// The release-control binaries rolled out to one target, as concrete paths the
/// reporter can hash.
///
/// A rollout product lives under its own install root — brama is
/// `/Users/charles/.stado/services/brama/bin/brama` — so it appears in neither
/// `$HOME/.stado/bin` nor any `managed_versions` entry, and a report that did not
/// name it could say nothing at all about the one binary
/// `stado release status` is about.
fn release_product_programs(
    document: &Value,
    host: &str,
) -> Vec<crate::host_software::ProductBinary> {
    let Ok(Some(control)) = crate::release_control::control(document) else {
        return Vec::new();
    };
    control
        .products
        .values()
        .filter(|policy| policy.targets.contains_key(host))
        .map(|policy| {
            let path = format!(
                "{}/{}",
                policy.install_root.trim_end_matches('/'),
                policy.binary.trim_start_matches('/')
            );
            crate::host_software::ProductBinary {
                name: path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&policy.binary)
                    .to_string(),
                path,
                desired: policy
                    .desired
                    .as_ref()
                    .map(|desired| desired.version.clone()),
            }
        })
        .collect()
}

/// `stado host software [TARGET] [--json]` — what a host actually runs, and
/// which of it came out of a release.
///
/// Naming a TARGET takes the report: one round trip over the audited channel,
/// and the answer is persisted as an observation before it is printed, so
/// `stado release status` can judge every rollout without opening an ssh
/// connection per target. Omitting TARGET reads what is already on file for
/// every host, ages included — a host that has never reported is absent from
/// that list and is reported as `never` by every gate that asks about it, which
/// is the state that used to print as `unreported` beside a zero exit.
///
/// The failure of the read is recorded too. Leaving the previous report in place
/// after a refused connection would let an hour-old answer keep reading as
/// current, which is the exact shape of the outage
/// [`crate::observations`] exists to make visible.
pub async fn software(target: Option<String>, json: bool) -> Result<(), CmdError> {
    let Some(target) = target else {
        let hosts = crate::host_software::reported_hosts(&crate::observations::load());
        return print_reports(&hosts, json).await;
    };
    let resolved = crate::deploy::host_channel::canonical_target(&target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let document = super::registry::fetch_document().await?;
    let products = release_product_programs(&document, &resolved.name);
    let programs: Vec<String> = products
        .iter()
        .map(|product| product.path.clone())
        .collect();
    let runner = crate::deploy::production_runner();
    match crate::host_software::gather(&resolved, &programs, &runner).await {
        Ok((rows, scripts)) => {
            crate::host_software::record(&resolved.name, &rows, scripts)
                .map_err(|error| CmdError::click(error.to_string()))?;
        }
        Err(error) => {
            // Recorded, then reported. An operator who is told the read failed
            // and finds yesterday's report still on file has been told two
            // different things by one command.
            if let Err(write) = crate::host_software::record_refusal(&resolved.name, &error.0) {
                eprintln!("warning: could not record the failed software read: {write}");
            }
            return Err(CmdError::click(format!(
                "{target}: cannot read what software it runs: {}",
                error.0
            )));
        }
    }
    print_reports(&[resolved.name], json).await
}

/// Every named host's newest report, with the disagreements the fleet has
/// against it.
async fn print_reports(hosts: &[String], json: bool) -> Result<(), CmdError> {
    let records = crate::observations::load();
    // The remote registry, because the declaration being checked is the fleet's
    // and not whatever a local copy last said. One fetch for every row: the
    // question "what does this host declare" is asked once per host and the
    // answer is one document.
    let registry = super::registry::read_registry().await.ok();
    let mut payload: Vec<Value> = Vec::new();
    let mut failures = usize::default();
    for host in hosts {
        let report = crate::host_software::load_in(&records, host);
        let declared = registry
            .as_ref()
            .and_then(|registry| registry.targets.iter().find(|entry| &entry.name == host))
            .map(|entry| entry.managed_versions.clone())
            .unwrap_or_default();
        let finding = crate::host_software::judge(&report, &declared, None);
        if finding.failed {
            failures = failures.saturating_add(1);
        }
        if json {
            let mut object = report.json();
            finding.merge_into(&mut object);
            payload.push(object);
            continue;
        }
        println!("{host}: {} [{}]", report.summary(), report.age());
        if !report.refusal().is_empty() {
            println!("  the last read did not complete: {}", report.refusal());
        }
        let rows: Vec<Vec<String>> = report
            .rows
            .iter()
            .map(|row| {
                vec![
                    row.provenance.clone(),
                    row.name.clone(),
                    row.version.clone(),
                    row.sha256.chars().take(12).collect(),
                    row.path.clone(),
                ]
            })
            .collect();
        if !rows.is_empty() {
            super::table::print(&["PROVENANCE", "NAME", "VERSION", "SHA256", "PATH"], &rows);
        }
        for sentence in &finding.sentences {
            println!("  ! {sentence}");
        }
    }
    if json {
        print_json(&json!({"hosts": payload}));
    } else if hosts.is_empty() {
        println!(
            "no host has reported its software: run `stado host software TARGET` for each \
             registry target, because a host that never says what it runs is not a host anything \
             here can vouch for"
        );
    }
    if failures == usize::default() {
        return Ok(());
    }
    // This command reports and it also gates, for the same reason
    // `stado release status` now does: printing a host that cannot be shown to
    // run what the fleet declares, and then exiting zero, is the shape of the
    // failure the whole report exists to end. Every sentence is already beside
    // the host it belongs to, so nothing is said twice.
    eprintln!(
        "{failures} of {} host(s) cannot be shown to be running what the fleet declares for \
         them; each is named above",
        hosts.len()
    );
    Err(CmdError::silent(super::CLICK_ERROR_CODE))
}
