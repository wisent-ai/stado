//! `stado host ...` — Rust implementations of the complete `host` group:
//! health, recovery, user provisioning, and Weles recordings policy, plus
//! the read-only diagnostics of `docs/missing-commands.md` items two
//! through six (`uptime`, `ping`, `disk`, `cleanup --dry-run`, `exec`),
//! which have no Python original and live in `crate::deploy::host_*`.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CString, OsStr};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

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

/// `stado host beacon-units` — the unit ids the registry declares for this
/// machine, one per line.
///
/// The list the health beacon must ask systemd or launchd about. Answered from
/// the registry rather than assembled in the collector, because the registry
/// is already the one place that says what a host runs and a second list in
/// shell would be a second answer to that question. That second list existed:
/// `WC_HEALTH_UNITS`, typed per host, and on ubuntu-server-rtx-pro-6000 it
/// named `wisent-agent.service` alone while the registry declared
/// `stado-resolver` there. The declared unit was never asked about, so the
/// beacon carried no entry for it and `registry doctor` reported it as a unit
/// the host does not have — while it was active with a live pid.
///
/// Never fails the caller. A machine that is not in the registry, or a
/// registry that cannot be read, prints nothing and exits zero: the beacon
/// then reports the operator's own list, and a collector that died here would
/// report nothing at all.
pub async fn beacon_units() -> Result<(), CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let Ok(Some(target)) = crate::providers::local::agent::lookup_self_auto(&hostname).await else {
        return Ok(());
    };
    for service in crate::deploy::service::declared_services(&target) {
        let unit = service.unit_id();
        // A unit id carrying a space or a comma would break the collector's
        // own comma-separated list, and nothing in this fleet has one.
        if unit.is_empty() || unit.contains(char::is_whitespace) || unit.contains(',') {
            continue;
        }
        println!("{unit}");
    }
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
    // The beacon's grant is an operator-provisioned file that stays where it is:
    // this runs on a schedule for the life of the host, so it must re-read and
    // pick up a rotated grant, and it must never erase the file it depends on.
    let client = crate::skarbiec::Client::new(
        url.trim(),
        &consumer,
        &token_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    )
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

/// `stado host recover TARGET [--release VERSION]` — optionally replace the
/// remote Stado binary from the registry-trusted signed emergency channel,
/// then recover a registry-managed macOS host through its approved channel.
///
/// The canonical remote registry remains the default and fleet-survival
/// authority. `bundled_registry` is an explicit break-glass path for repairing
/// the storage or authorization outage that made that authority unreadable.
/// The selected registry is loaded exactly once: release trust and host
/// identity must come from the same last-known-good or explicit bundled copy.
pub async fn recover(
    target: &str,
    bundled_registry: bool,
    release: Option<&str>,
) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let registry = if bundled_registry {
        crate::targets::load_bundled_registry().map_err(|exc| CmdError::click(exc.to_string()))?
    } else {
        crate::deploy::host_channel::canonical_registry()
            .await
            .map_err(|exc| CmdError::click(exc.to_string()))?
    };
    let (report, object_api) = match release {
        Some(version) => {
            if registry.lookup(target).is_none() {
                return Err(CmdError::click(format!("target not in registry: {target}")));
            }
            // A signed recovery release is fetched through the object API.
            // Repair that authority first without depending on the authority
            // itself. The service directory, not the host being recovered,
            // names where that shared API runs.
            let object_api_host = registry
                .service(OBJECT_API_SERVICE)
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "service directory declares no {OBJECT_API_SERVICE}; refusing to guess \
                         which host owns release-object recovery"
                    ))
                })?
                .active_host
                .clone();
            let object_api_target =
                crate::deploy::host_channel::resolve_target(&registry, &object_api_host)
                    .map_err(|error| CmdError::click(error.to_string()))?;
            let object_api = recover_object_api_on_target(object_api_target, &runner).await?;
            (
                crate::deploy::host_recovery_release::recover(&registry, target, version, &runner)
                    .await,
                Some(object_api),
            )
        }
        None => (
            crate::deploy::host_recovery::recover_host_with_registry(&registry, target, &runner)
                .await,
            None,
        ),
    };
    let mut report = report.map_err(|exc| CmdError::click(exc.to_string()))?;
    if let (Some(object), Some(detail)) = (report.as_object_mut(), object_api) {
        object.insert(
            "object_api".to_string(),
            json!({"status": "healthy", "detail": detail}),
        );
    }
    println!(
        "{}",
        crate::deploy::host_recovery::to_sorted_pretty(&report)
    );
    if report.get("status").and_then(Value::as_str) != Some(crate::deploy::host_recovery::STATUS_OK)
    {
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
    let report = crate::deploy::host_gui_automation::status(&resolved, &runner).await;
    print_report(&report)
}

/// `stado host gui-automation enable TARGET` — configure persistent GUI login,
/// install the pinned signed CuaDriver app and grant Accessibility.
pub async fn gui_automation_enable(target: &str) -> Result<(), CmdError> {
    let resolved = registry_target(target).await?;
    let password = super::service::host_sudo_password(&resolved)
        .await?
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} has no readable host-account password",
                resolved.name
            ))
        })?;
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_gui_automation::enable(&resolved, &password, &runner).await;
    print_report(&report)
}

/// `stado host gui-automation grant-accessibility TARGET` — grant the
/// installed, signed CuaDriver app Accessibility for the host's GUI user.
pub async fn gui_automation_grant_accessibility(target: &str) -> Result<(), CmdError> {
    let resolved = registry_target(target).await?;
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_gui_automation::grant_accessibility(&resolved, &runner).await;
    print_report(&report)
}

/// `stado host gui-automation disable TARGET [--bundle ID]` — revert the
/// enablement and report every item it touched.
pub async fn gui_automation_disable(target: &str, bundle: &str) -> Result<(), CmdError> {
    let resolved = registry_target(target).await?;
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_gui_automation::disable(&resolved, bundle, &runner).await;
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

/// Every field `stado host disk-cleanup` may rewrite, as parsed from argv.
///
/// One field per registry key, because the policy is the whole contract
/// between an operator and the janitor and a setter that covered part of it
/// would leave the rest editable only by hand. Before this existed, `stado`
/// could set exactly one of these — `host weles-recordings-dir`, one
/// cleaner's root — and the dashboard accepted exactly one more, `mode`, so a
/// watermark, a budget or a `build_caches` root could only be changed by
/// pulling `registry.json`, editing it, and pushing it back.
pub struct DiskCleanupPolicyEdit {
    pub mode: Option<String>,
    pub check_interval_seconds: Option<i64>,
    pub low_free_gb: Option<i64>,
    pub target_free_gb: Option<i64>,
    pub max_items_per_pass: Option<i64>,
    pub max_bytes_per_pass: Option<i64>,
    pub max_scan_items: Option<i64>,
    pub max_pass_seconds: Option<i64>,
    pub clear_max_pass_seconds: bool,
    pub add_cleaner: Vec<String>,
    pub remove_cleaner: Vec<String>,
    pub cleaner_root: Vec<String>,
    pub clear_cleaner_root: Vec<String>,
    pub cleaner_min_age_seconds: Vec<String>,
}

impl DiskCleanupPolicyEdit {
    /// Whether argv asked for a read rather than a write.
    fn is_read_only(&self) -> bool {
        self.mode.is_none()
            && self.check_interval_seconds.is_none()
            && self.low_free_gb.is_none()
            && self.target_free_gb.is_none()
            && self.max_items_per_pass.is_none()
            && self.max_bytes_per_pass.is_none()
            && self.max_scan_items.is_none()
            && self.max_pass_seconds.is_none()
            && !self.clear_max_pass_seconds
            && self.add_cleaner.is_empty()
            && self.remove_cleaner.is_empty()
            && self.cleaner_root.is_empty()
            && self.clear_cleaner_root.is_empty()
            && self.cleaner_min_age_seconds.is_empty()
    }
}

/// `NAME=VALUE` from one repeatable flag.
fn cleaner_pair(raw: &str, flag: &str) -> Result<(String, String), CmdError> {
    let Some((name, value)) = raw.split_once('=') else {
        return Err(CmdError::usage(format!(
            "--{flag} takes NAME=VALUE, got {raw:?}"
        )));
    };
    let (name, value) = (name.trim().to_string(), value.trim().to_string());
    if name.is_empty() || value.is_empty() {
        return Err(CmdError::usage(format!(
            "--{flag} takes NAME=VALUE, got {raw:?}"
        )));
    }
    Ok((name, value))
}

/// The retention floor `targets::validate_registry` enforces for one cleaner,
/// used as the age gate a newly enabled cleaner starts with. Read from the
/// same three cases the validator states, so enabling a cleaner cannot
/// produce a document the validator then refuses.
fn cleaner_age_floor(name: &str) -> i64 {
    match name {
        "huggingface_cache" => 3600,
        "queue_workdirs" | "backup_twins" => 0,
        _ => 86400,
    }
}

/// `serde` writes `Option::None` as `null`, and the cleaner schema accepts a
/// key list rather than nulls, so a seeded default is stripped before it is
/// validated. Applied only to the policy subtree this command builds.
fn strip_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, entry| !entry.is_null());
            for entry in map.values_mut() {
                strip_nulls(entry);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

/// Read or rewrite one target's `disk_cleanup` policy.
///
/// The write is the same compare-and-swap every registry setter here uses:
/// read the current generation, rewrite exactly the named fields, validate the
/// WHOLE document, and swap it only if nobody else moved it. A target that
/// declares no policy is seeded from
/// [`crate::targets::DiskCleanupPolicy::reporting_default`] first, so its
/// first declaration starts at `report` rather than at whatever the flags
/// happen to omit.
pub async fn disk_cleanup_policy(
    target: &str,
    edit: DiskCleanupPolicyEdit,
    json: bool,
) -> Result<(), CmdError> {
    if edit.max_pass_seconds.is_some() && edit.clear_max_pass_seconds {
        return Err(CmdError::usage(
            "--max-pass-seconds and --clear-max-pass-seconds are mutually exclusive",
        ));
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

    if edit.is_read_only() {
        let declared = entry.get("disk_cleanup").cloned();
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "target": target,
                    "generation": current.version,
                    "declared": declared.is_some(),
                    "disk_cleanup": declared,
                }))?
            );
        } else if let Some(policy) = declared {
            println!("{target}: {}", serde_json::to_string_pretty(&policy)?);
        } else {
            println!(
                "{target}: declares no disk_cleanup policy; it is measured against the \
                 reporting default, which reports and never deletes"
            );
        }
        return Ok(());
    }

    let mut policy = match entry.get("disk_cleanup") {
        Some(existing) if existing.is_object() => existing.clone(),
        _ => {
            let mut seeded =
                serde_json::to_value(crate::targets::DiskCleanupPolicy::reporting_default())?;
            strip_nulls(&mut seeded);
            seeded
        }
    };
    let policy_map = policy
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry target disk_cleanup must be an object"))?;

    for (key, declared) in [
        ("mode", edit.mode.clone().map(Value::from)),
        (
            "check_interval_seconds",
            edit.check_interval_seconds.map(Value::from),
        ),
        ("low_free_gb", edit.low_free_gb.map(Value::from)),
        ("target_free_gb", edit.target_free_gb.map(Value::from)),
        (
            "max_items_per_pass",
            edit.max_items_per_pass.map(Value::from),
        ),
        (
            "max_bytes_per_pass",
            edit.max_bytes_per_pass.map(Value::from),
        ),
        ("max_scan_items", edit.max_scan_items.map(Value::from)),
        ("max_pass_seconds", edit.max_pass_seconds.map(Value::from)),
    ] {
        if let Some(value) = declared {
            policy_map.insert(key.to_string(), value);
        }
    }
    if edit.clear_max_pass_seconds {
        policy_map.remove("max_pass_seconds");
    }

    let cleaners = policy_map
        .entry("cleaners".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .ok_or_else(|| CmdError::click("disk_cleanup.cleaners must be an object"))?;

    // Enabling a cleaner writes the retention floor its own schema demands,
    // so `--cleaner build_caches` produces a document that validates rather
    // than one refused for a missing `min_age_seconds`.
    let enable = |name: &str, cleaners: &mut serde_json::Map<String, Value>| {
        cleaners
            .entry(name.to_string())
            .or_insert_with(|| json!({ "min_age_seconds": cleaner_age_floor(name) }));
    };
    for name in &edit.add_cleaner {
        enable(name, cleaners);
    }
    for name in &edit.remove_cleaner {
        cleaners.remove(name);
    }
    for raw in &edit.cleaner_root {
        let (name, path) = cleaner_pair(raw, "cleaner-root")?;
        enable(&name, cleaners);
        cleaners
            .get_mut(&name)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(format!("disk_cleanup.cleaners.{name} must be an object"))
            })?
            .insert("root".to_string(), Value::from(path));
    }
    for name in &edit.clear_cleaner_root {
        if let Some(cleaner) = cleaners.get_mut(name).and_then(Value::as_object_mut) {
            cleaner.remove("root");
        }
    }
    for raw in &edit.cleaner_min_age_seconds {
        let (name, raw_seconds) = cleaner_pair(raw, "cleaner-min-age-seconds")?;
        let seconds: i64 = raw_seconds.parse().map_err(|_| {
            CmdError::usage(format!(
                "--cleaner-min-age-seconds takes NAME=SECONDS, got {raw:?}"
            ))
        })?;
        enable(&name, cleaners);
        cleaners
            .get_mut(&name)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(format!("disk_cleanup.cleaners.{name} must be an object"))
            })?
            .insert("min_age_seconds".to_string(), Value::from(seconds));
    }

    entry.insert("disk_cleanup".to_string(), policy.clone());
    // The whole registry, not the field: a policy is only valid in the
    // document that carries it, and the janitor refuses a document that does
    // not validate as a whole. Remove declarations the current model
    // intentionally retired (`slots`, `max_concurrent`, and
    // `WC_LOCAL_SLOTS`) on this ordinary policy update instead of retaining a
    // second capacity contract.
    crate::targets::strip_legacy_capacity_declarations(&mut document);
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let payload = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let generation = store.compare_and_swap(&current.version, &payload).await?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "generation": generation,
                "disk_cleanup": policy,
            }))?
        );
    } else {
        println!("{target}: {}", serde_json::to_string_pretty(&policy)?);
        println!("generation {generation}");
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
        // Whose verdict this is. Several processes write that one file on an
        // always-on host -- the queue agent every tick, a `disk-cleanup
        // --watch` unit on its own timer -- so OUTCOME above is the last pass
        // by whoever made it, not a property of the host. On 2026-08-31 the
        // agent recorded `interval_noop` with no errors and this command read
        // `invalid_or_unavailable_policy` 46 seconds later from the same path.
        // Naming the writer is what lets an operator tell those apart instead
        // of believing whichever arrived last.
        if let Some(Value::String(writer)) = state.and_then(|value| value.get("writer")) {
            let version = state
                .and_then(|value| value.get("writer_version"))
                .and_then(Value::as_str)
                .unwrap_or("unknown version");
            println!(
                "that pass was written by {writer} running {version}; this file \
                 has more than one writer, so OUTCOME is the last pass rather \
                 than the state of the host"
            );
        }
        // Which declared cleaners the pass never reached. `cap_reached` above
        // says a budget stopped the pass and cannot say whom it stopped, and
        // the per-cleaner table prints the same three zeros for a cleaner that
        // never got a turn as for one that looked and found nothing. On
        // charless-mac-mini `backup_twins` sat at zeros under real pressure
        // for as long as anyone had looked, behind a `build_caches` walk of
        // the whole of `$HOME`, while the host refused every ordinary job.
        let unscanned: Vec<&str> = state
            .and_then(|value| value.get("unscanned_cleaners"))
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !unscanned.is_empty() {
            println!(
                "the pass ended before these declared cleaner(s) scanned anything: {} — raise \
                 `max_pass_seconds` or `max_scan_items` with `stado host disk-cleanup`, or narrow \
                 an earlier cleaner's root, or their zeros mean nobody looked",
                unscanned.join(", ")
            );
        }
    } else {
        println!(
            "\ncleanup state: no state file at {} — the janitor has never \
             completed a pass on this host",
            recorded("path")
        );
    }
    // Who holds the run lock, printed with the state it explains. A pass that
    // reported `lock_busy`, and an agent publishing `cleanup_in_progress`, are
    // both this one fact seen from the outside; until this line existed an
    // operator could read either of them for hours with no way to learn which
    // process to look at.
    let lock = report.get("cleanup_lock");
    let lock_read = lock.and_then(|value| value.get("read")) == Some(&Value::Bool(true));
    let holders: Vec<&Value> = lock
        .and_then(|value| value.get("holders"))
        .and_then(Value::as_array)
        .map(|rows| rows.iter().collect())
        .unwrap_or_default();
    if lock_read && !holders.is_empty() {
        let cells: Vec<Vec<String>> = holders
            .iter()
            .map(|holder| {
                vec![
                    cell(holder.get("pid")),
                    cell(holder.get("command")),
                    cell(lock.and_then(|value| value.get("path"))),
                ]
            })
            .collect();
        println!("\nthe janitor's run lock is held — no pass can scan while it is:");
        super::table::print(&["PID", "COMMAND", "LOCK"], &cells);
    } else if lock_read {
        println!("\nthe janitor's run lock is free");
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

/// `stado host object-relocate TARGET --namespace NS --from-prefix P
/// [--to-prefix Q] [--apply]` — re-address objects inside the store, on the
/// host that holds it.
///
/// The refusals are printed last and printed always, because they are the
/// only lines an operator has to act on: an object whose destination exists
/// with different bytes is still at its wrong address and still has a second
/// copy, and a run that reports 88 moves and hides one of those reads as a
/// completed repair.
pub async fn object_relocate(
    target: &str,
    plan: &crate::deploy::host_object_relocate::RelocatePlan,
    json: bool,
) -> Result<(), CmdError> {
    let apply = plan.apply;
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_object_relocate::relocate_host(target, plan, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let expected = crate::deploy::host_object_relocate::OK_STATUS;
    if json {
        print_json(&report);
        return report_outcome(&report, expected);
    }
    let store = report.get("store");
    let named = |key: &str| cell(store.and_then(|value| value.get(key)));
    if let Some(Value::String(root)) = store.and_then(|value| value.get("missing_root")) {
        println!("no store at {root} on this host — nothing was read");
        return report_outcome(&report, expected);
    }
    if let Some(Value::String(os)) = store.and_then(|value| value.get("no_hasher")) {
        println!(
            "no sha256 program on this {os} host, so no body could be verified and \
             none was touched"
        );
        return report_outcome(&report, expected);
    }
    println!(
        "store {}\n  from {}\n    to {}",
        named("root"),
        named("source_prefix"),
        named("destination_prefix"),
    );
    let totals = report.get("totals");
    let counted = |key: &str| {
        totals
            .and_then(|value| value.get(key))
            .and_then(Value::as_i64)
            .unwrap_or_default()
    };
    let objects: Vec<&Value> = report
        .get("objects")
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default();
    let field = |item: &Value, key: &str| cell(item.get(key));
    super::table::print(
        &["OUTCOME", "BYTES", "SOURCE KEY", "DESTINATION KEY"],
        &objects
            .iter()
            .map(|item| {
                vec![
                    field(item, "outcome"),
                    field(item, "bytes"),
                    field(item, "source_key"),
                    field(item, "destination_key"),
                ]
            })
            .collect::<Vec<_>>(),
    );
    println!(
        "\n{} scanned, {} decided, {} relocated ({:.2} GiB), {} refused, {} empty \
         directories pruned",
        counted("scanned"),
        counted("decided"),
        counted("moved"),
        counted("moved_bytes") as f64 / 1024.0_f64.powi(3),
        counted("refused"),
        counted("pruned_directories"),
    );
    // Said as its own line rather than folded into the counts above, because
    // it is a different repair: the body is at the right address and the
    // sidecar beside it still records the wrong one, which is what
    // `storage ls --long` reads out.
    let stale = counted("stale_uris");
    if stale > 0 {
        println!(
            "{stale} sidecars still record the old address, {} rewritten",
            counted("repaired_uris"),
        );
    }
    if !apply {
        println!("nothing was changed: pass --apply to relocate what is listed above");
    }
    // A pass the host cut short states so rather than letting its totals read
    // as the whole tree.
    if totals.and_then(|value| value.get("complete")) != Some(&Value::Bool(true)) {
        println!(
            "the host's closing count never arrived, so these totals are a lower bound; \
             run the command again"
        );
    }
    let remaining = counted("remaining");
    if remaining > 0 {
        println!("{remaining} left under the source prefix; run the command again to continue");
    }
    for item in &objects {
        let outcome = item.get("outcome").and_then(Value::as_str).unwrap_or("");
        if crate::deploy::host_object_relocate::is_refusal(outcome) {
            println!(
                "  {outcome}: {} still holds its own bytes and was left where it is",
                field(item, "source_key")
            );
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
    // Both stores on one line, ahead of the capacity line the first one
    // explains: an agent bound to a device-local store publishes capacity into
    // a store nothing here reads, and an operator who cannot see the two
    // backend names side by side reads `capacity_publication_stale` and goes
    // looking at the agent's uptime instead of at what its unit exports.
    match gates.agent_store_backend.as_deref() {
        Some(backend) => println!(
            "store:    agent writes to {backend}, this control plane reads {}{}",
            gates.fleet_store_backend,
            store_clause(&gates.blockers),
        ),
        None => println!(
            "store:    this host did not answer with a storage backend, so where its agent \
             publishes cannot be shown; this control plane reads {}",
            gates.fleet_store_backend
        ),
    }
    match gates.published_at.as_deref() {
        Some(published) => {
            let admission = match gates.accepting_jobs {
                Some(true) => "accepting jobs",
                Some(false) => "busy or gated",
                None => "admission unstated",
            };
            let cpu = gates
                .available_cpu_cores
                .zip(gates.total_cpu_cores)
                .map_or_else(
                    || "-/-".to_string(),
                    |(free, total)| format!("{free}/{total}"),
                );
            let ram = gates.free_ram_gb.zip(gates.total_ram_gb).map_or_else(
                || "-/-".to_string(),
                |(free, total)| format!("{free:.1}/{total:.1}"),
            );
            let vram = gates.free_vram_gb.zip(gates.total_vram_gb).map_or_else(
                || "-/-".to_string(),
                |(free, total)| format!("{free}/{total}"),
            );
            println!(
                "capacity: {admission}, {} running job(s), CPU {cpu} cores available/total, \
                 RAM {ram} GiB free/total, VRAM {vram} GiB free/total; published {} ({published})",
                gates.running_jobs.unwrap_or_default(),
                gates.age_seconds.map_or_else(
                    || "at an unknown time".to_string(),
                    |age| format!(
                        "{} ago",
                        super::registry::human_age(chrono::TimeDelta::seconds(age))
                    )
                ),
            );
            if !gates.available_accelerators.is_empty() {
                println!(
                    "accelerators: {}",
                    serde_json::to_string(&gates.available_accelerators)
                        .unwrap_or_else(|_| "{}".to_string())
                );
            }
        }
        None => {
            println!("capacity: nothing published for this host, so the scheduler cannot see it")
        }
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
                "note:     {note} — {} local APFS snapshot(s), which macOS reports no size \
                 for. `stado host reclaim {}` names each one and why it refuses it; the \
                 `com.apple.os.update-*` ones are OS-update snapshots rather than local Time \
                 Machine snapshots, so no stado command deletes them and none should",
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

/// What the two backend names mean when they do not agree, read off the
/// blocker [`crate::deploy::host_gates`] already decided.
///
/// Keyed off the blocker and never re-classified here: a second classifier of
/// storage backends in the CLI would eventually disagree with the one in the
/// reader about one host, and the operator would believe whichever line they
/// read first.
fn store_clause(blockers: &[String]) -> &'static str {
    if blockers
        .iter()
        .any(|blocker| blocker == crate::deploy::host_gates::AGENT_STORE_DEVICE_ONLY)
    {
        return " — a store only that host can address, so nothing its agent publishes ever \
                reaches this fleet";
    }
    if blockers
        .iter()
        .any(|blocker| blocker == crate::deploy::host_gates::AGENT_STORE_UNKNOWN)
    {
        return " — a backend this build has no adapter for, so how far that agent's writes \
                carry cannot be decided here";
    }
    ""
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
const HOST_HEALTH_BEACON_UNIT: &str = "com.wisent.host-health-beacon";
const HOST_HEALTH_AUTH_UNAVAILABLE: &str = "host-health authorization unavailable";
const HOST_HEALTH_LOG_LINES: u32 = 80;
const OBJECT_API_SERVICE: &str = "stado-object-api";
const LINK_REPAIR_WAIT_SECONDS: u64 = 90;
const LINK_REPAIR_POLL_SECONDS: u64 = 5;

/// The beacon publisher's newest outcome, read from the managed unit's own
/// declared log. A later successful host-name line supersedes an older error;
/// otherwise an incident that was already repaired would keep advertising the
/// old cause forever.
fn host_health_publisher_diagnosis(report: &UnitLogReport) -> Value {
    let lines = report.log.lines().collect::<Vec<_>>();
    let last_success = lines.iter().rposition(|line| line.trim() == report.target);
    let last_error = lines
        .iter()
        .rposition(|line| line.contains("Error:") || line.contains(HOST_HEALTH_AUTH_UNAVAILABLE));
    if matches!(
        (last_success, last_error),
        (Some(success), Some(error)) if success > error
    ) || matches!((last_success, last_error), (Some(_), None))
    {
        return json!({
            "unit": report.unit,
            "code": "published",
            "detail": "The beacon publisher's newest recorded attempt succeeded.",
            "repairable": false,
        });
    }
    if let Some(index) = last_error {
        if lines[index].contains(HOST_HEALTH_AUTH_UNAVAILABLE) {
            return json!({
                "unit": report.unit,
                "code": "verifier_unavailable",
                "detail": format!(
                    "The beacon publisher reached the host-health API, but that API could not read \
                     {}/token through its dedicated verifier.",
                    crate::config::HOST_HEALTH_API_ITEM
                ),
                "repairable": true,
                "repair_command": format!("stado host repair-link {}", report.target),
            });
        }
        return json!({
            "unit": report.unit,
            "code": "publisher_failed",
            "detail": lines[index].trim(),
            "repairable": false,
        });
    }
    json!({
        "unit": report.unit,
        "code": "publisher_silent",
        "detail": "The beacon is stale, the host answers, and its publisher log names no failed publish.",
        "repairable": false,
    })
}

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

    // Probe every declared route so a working primary does not hide a broken
    // fallback. The real command below still chooses in declaration order and
    // runs once.
    let (connection_probes, connection_probe_error) =
        match crate::deploy::host_channel::probe_ssh_connections(resolved, &runner).await {
            Ok(probes) => (probes, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
    let connection_degraded =
        connection_probe_error.is_some() || connection_probes.iter().any(|probe| !probe.reachable);
    let ssh = crate::deploy::host_channel::run_program_with_connection(
        resolved,
        crate::deploy::host_ping::REMOTE_PROGRAM,
        &runner,
    )
    .await;
    let (ssh_reachable, ssh_error, selected_connection) = match ssh {
        Ok((output, used)) if output.ok() => {
            let selected = match used {
                crate::deploy::host_channel::UsedConnection::Local => "local".to_string(),
                crate::deploy::host_channel::UsedConnection::Ssh(connection) => {
                    connection.name.to_string()
                }
            };
            (true, None, Some(selected))
        }
        Ok((output, _)) => (
            false,
            Some(crate::deploy::host_channel::last_error_line(
                &output,
                "ssh failed",
            )),
            None,
        ),
        Err(exc) => (false, Some(exc.to_string()), None),
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
    // A reachable host with a stale beacon is a publisher failure, not a
    // network mystery. Read the managed publisher's own log here so `link`
    // carries the cause an operator previously had to discover with a second
    // command.
    let beacon_publisher = if stale && ssh_reachable {
        match collect_unit_log(
            resolved,
            HOST_HEALTH_BEACON_UNIT,
            HOST_HEALTH_LOG_LINES,
            &runner,
        )
        .await
        {
            Ok(report) => Some(host_health_publisher_diagnosis(&report)),
            Err(error) => Some(json!({
                "unit": HOST_HEALTH_BEACON_UNIT,
                "code": "diagnostic_unavailable",
                "detail": error.to_string(),
                "repairable": false,
            })),
        }
    } else {
        None
    };
    if let Some(detail) = beacon_publisher
        .as_ref()
        .and_then(|publisher| publisher.get("detail"))
        .and_then(Value::as_str)
    {
        blockers.push(detail.to_string());
    }

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
    if let Some(detail) = &connection_probe_error {
        blockers.push(format!("connection path probes failed: {detail}"));
    }
    for probe in connection_probes.iter().filter(|probe| !probe.reachable) {
        blockers.push(format!(
            "{} connection path {} did not answer: {}",
            probe.name,
            probe.destination,
            probe.error.as_deref().unwrap_or("SSH probe failed")
        ));
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
    } else if refused || connection_degraded {
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
        "selected_connection": &selected_connection,
        "connection_paths": &connection_probes,
        "connection_probe_error": &connection_probe_error,
        "session": session.to_json(),
        "beacon_publisher": &beacon_publisher,
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
    if let Some(publisher) = &beacon_publisher {
        println!(
            "publisher:{}",
            publisher.get("detail").and_then(Value::as_str).map_or_else(
                || " no diagnosis".to_string(),
                |detail| format!(" {detail}")
            )
        );
    }
    println!(
        "ssh:      {}",
        match &ssh_error {
            None => "answered".to_string(),
            Some(detail) => format!("did not answer: {detail}"),
        }
    );
    if let Some(detail) = &connection_probe_error {
        println!("routes:   could not probe: {detail}");
    } else {
        for (index, probe) in connection_probes.iter().enumerate() {
            let label = if index == 0 { "routes:  " } else { "         " };
            let selected = if selected_connection.as_deref() == Some(probe.name.as_str()) {
                ", selected"
            } else {
                ""
            };
            let state = if probe.reachable {
                format!("answered{selected}")
            } else {
                format!(
                    "did not answer: {}",
                    probe.error.as_deref().unwrap_or("SSH probe failed")
                )
            };
            println!("{label} {} ({}) {state}", probe.name, probe.destination);
        }
    }
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

/// Repair one stale, reachable host whose publisher is being refused because
/// the dashboard cannot read its route-scoped bearer.
///
/// The repair is deliberately narrow. It reads the publisher's own declared
/// log, refuses every other cause, resolves the object API authority from the
/// service directory, copies the authoritative route bearer into that
/// authority's target-local verifier shadow, reconciles the existing grant
/// without rotating its bearer, then waits for the host's normal one-minute
/// publisher to prove the repair with a newer beacon. No service is restarted.
pub async fn repair_link(target: &str, json_output: bool) -> Result<(), CmdError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let resolved = crate::deploy::host_channel::resolve_target(&registry, target)
        .map_err(|error| CmdError::click(error.to_string()))?
        .clone();
    let store = beacon_store().await?;
    let initial_health = crate::monitor::host_health::load_host_health(&store, &resolved.name)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let initial_signal =
        crate::deploy::host_ping::grade_beacon(&initial_health, chrono::Utc::now());
    let threshold = crate::monitor::host_silence::silence_threshold_seconds();
    if initial_signal
        .age_seconds
        .is_some_and(|age| age <= threshold)
    {
        let report = json!({
            "target": resolved.name,
            "state": "already_healthy",
            "detail": "The newest beacon is inside the fleet silence threshold; no repair changed the verifier.",
            "beacon_age_seconds": initial_signal.age_seconds,
            "beacon_reported_at": initial_signal.reported_at,
        });
        if json_output {
            print_json(&report);
        } else {
            println!(
                "{}: beacon is already fresh; no repair changed the verifier",
                resolved.name
            );
        }
        return Ok(());
    }

    let runner = crate::deploy::production_runner();
    let publisher_log = collect_unit_log(
        &resolved,
        HOST_HEALTH_BEACON_UNIT,
        HOST_HEALTH_LOG_LINES,
        &runner,
    )
    .await?;
    let diagnosis = host_health_publisher_diagnosis(&publisher_log);
    let diagnosis_code = diagnosis
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if diagnosis_code != "verifier_unavailable" {
        let detail = diagnosis
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("the publisher log names no supported repair");
        return Err(CmdError::click(format!(
            "{}: automatic link repair refused because the diagnosed publisher state is \
             {diagnosis_code}: {detail}",
            resolved.name
        )));
    }

    let authority = registry
        .service(OBJECT_API_SERVICE)
        .ok_or_else(|| {
            CmdError::click(format!(
                "service directory declares no {OBJECT_API_SERVICE}; refusing to guess which \
                 host owns host-health authorization"
            ))
        })?
        .active_host
        .clone();
    crate::deploy::host_channel::resolve_target(&registry, &authority)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let verifier = reconcile_object_verifier_report(&authority).await?;

    let previous_reported_at = initial_signal.reported_at.clone();
    let started = std::time::Instant::now();
    let mut last_observation = format!("beacon remained {:?}s old", initial_signal.age_seconds);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(LINK_REPAIR_POLL_SECONDS)).await;
        match crate::monitor::host_health::load_host_health(&store, &resolved.name).await {
            Ok(health) => {
                let signal = crate::deploy::host_ping::grade_beacon(&health, chrono::Utc::now());
                last_observation = format!(
                    "newest beacon is {:?}s old and was reported at {}",
                    signal.age_seconds,
                    signal.reported_at.as_deref().unwrap_or("an unknown time")
                );
                let newer = signal.reported_at != previous_reported_at;
                let fresh = signal.age_seconds.is_some_and(|age| age <= threshold);
                if newer && fresh {
                    let newest_beacon_at = signal
                        .reported_at
                        .as_deref()
                        .and_then(crate::deploy::host_ping::parse_timestamp);
                    crate::monitor::host_silence::observe_beacon_age(
                        &store,
                        &resolved.name,
                        newest_beacon_at,
                        crate::monitor::host_silence::READER_CLI,
                        None,
                    )
                    .await
                    .map_err(|error| CmdError::click(error.to_string()))?;
                    let silences = crate::monitor::host_silence::recent_silences(
                        &store,
                        &resolved.name,
                        NEWEST_SILENCES,
                    )
                    .await
                    .map_err(|error| CmdError::click(error.to_string()))?;
                    let silence_closed = silences.iter().all(|record| record.ended_at.is_some());
                    let report = json!({
                        "target": resolved.name,
                        "state": "repaired",
                        "detail": if silence_closed {
                            "The verifier was reconciled, the host published a fresh beacon, and its open silence is closed."
                        } else {
                            "The verifier was reconciled and the host published a fresh beacon."
                        },
                        "authority": authority,
                        "diagnosis": diagnosis,
                        "verifier": verifier,
                        "previous_beacon_reported_at": previous_reported_at,
                        "beacon_reported_at": signal.reported_at,
                        "beacon_age_seconds": signal.age_seconds,
                        "silence_closed": silence_closed,
                        "waited_seconds": started.elapsed().as_secs(),
                    });
                    if json_output {
                        print_json(&report);
                    } else {
                        println!(
                            "{}: dashboard verifier reconciled on {}; a fresh beacon arrived \
                             and the open silence is {}",
                            resolved.name,
                            authority,
                            if silence_closed {
                                "closed"
                            } else {
                                "not recorded"
                            }
                        );
                    }
                    return Ok(());
                }
            }
            Err(error) => {
                last_observation = error.to_string();
            }
        }
        if started.elapsed().as_secs() >= LINK_REPAIR_WAIT_SECONDS {
            return Err(CmdError::click(format!(
                "{}: reconciled the dashboard verifier on {authority}, but no fresh beacon \
                 arrived within {LINK_REPAIR_WAIT_SECONDS}s; {last_observation}",
                resolved.name
            )));
        }
    }
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
    // A skipped stage is an infrastructure failure that the command survived,
    // so the classification line `main_entry` emits on the error path never
    // fires for it — and a stage nobody could judge, reported only as a row
    // in a human table, is the silence that cost the release train three
    // attempts. The store's own sentence rides in the reason, so `HTTP 502`
    // classes as `infra_down`, `retryable=true` here instead of the
    // `unknown`, `retryable=false` a discarded error used to produce. The
    // point and service are the two `cli/mod.rs` would derive for this
    // command: `failure_point` walks the subcommand names, `failure_service`
    // maps `host` to `fleet`.
    for (stage, reason) in &reclamation.skipped {
        crate::failure::log_failure(
            "cli.host.reclaim",
            "fleet",
            crate::failure::classify_message(reason),
            &format!("{stage}: {reason}"),
        );
    }
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
    // A stage that could not be judged is not a stage that found nothing.
    // Printed beside the table, because the table's zero is the same digit a
    // clean host prints.
    for (stage, reason) in &reclamation.skipped {
        println!("{stage}: SKIPPED — {reason}");
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
///
/// The failure keeps whatever
/// [`crate::deploy::host_exec::ExecRefusal`] already knew about itself:
/// an allowlist refusal reaches the operator as the code it stated, its
/// approved spellings as help beside it, and — when `--json` was asked
/// for — as a document rather than as prose the caller cannot parse.
pub async fn exec(target: &str, words: Vec<String>, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_exec::exec_host(target, &words, &runner)
        .await
        .map_err(|exc| {
            let mut error = CmdError::click(exc.message).machine_readable(json);
            if let Some(code) = exc.code {
                error = error.stating(code);
            }
            if let Some(help) = exc.help {
                error = error.helping(help);
            }
            error
        })?;
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
    // Skarbiec validates the generated public key by spawning `openssl`.
    // Give that child the same implementation selected above; otherwise
    // macOS resolves `/usr/bin/openssl`, which rejects Homebrew's Ed25519 key.
    let openssl_search_path = openssl
        .rsplit_once('/')
        .map(|(directory, _)| {
            format!("{directory}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        })
        .unwrap_or_else(|| {
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_string()
        });

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
            "PATH={} SKARBIEC_VAULT_FILE={} {} token-register-acquisitions {} \
             --workload-public-key-file {} --replace-capabilities >/dev/null",
            crate::deploy::shlex_quote(&openssl_search_path),
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

/// The schema both halves of the Spis/Weles bridge require of the public
/// receipt-trust document.
const SPIS_TRUST_SCHEMA: &str = "wisent.spis-weles-receipt-trust.v1";

/// The one browser action the Spis admission binding grants.
const SPIS_TRUST_ACTION: &str = "generic_browser_task";

/// Exactly the fields the document carries. A sixth would be refused by the
/// consumer's `deny_unknown_fields` deserializer, so it is refused here first.
const SPIS_TRUST_FIELDS: &[&str] = &[
    "schema",
    "organizationId",
    "allowedAction",
    "receiptKeys",
    "keySetVersion",
];

/// The managed Skarbiec units whose own environment names the vault the
/// daemon actually serves, at the system paths the fleet installs them.
///
/// Which file is live is a property of the running daemon, not a default: a
/// host carries several vault files and `skarbiec` without
/// `SKARBIEC_VAULT_FILE` picks one that may hold nothing. Reading the unit is
/// how that question gets answered against the host rather than against a
/// guess.
const SKARBIEC_UNIT_PLISTS: &[&str] = &[
    "/Library/LaunchDaemons/com.wisent.always-on.skarbiec.plist",
    "/Library/LaunchAgents/com.wisent.always-on.skarbiec.plist",
];

/// The environment the Skarbiec daemon on this host is actually started with.
///
/// The vault is only the half that decides WHICH secrets answer. Skarbiec
/// decrypts by spawning GnuPG, so `GNUPGHOME` decides WHETHER any of them do,
/// and a read that inherits the vault without the keyring fails on the first
/// field with GnuPG's own "No such file or directory". Both come from the same
/// unit, so both are taken from it rather than one being read and the other
/// assumed.
async fn live_skarbiec_environment(
    resolved: &ComputeTarget,
    home: &str,
    runner: &crate::deploy::Runner,
) -> Result<Vec<(String, String)>, String> {
    use crate::deploy::host_channel;

    let mut units: Vec<String> = SKARBIEC_UNIT_PLISTS
        .iter()
        .map(|path| (*path).to_string())
        .collect();
    units.push(format!(
        "{home}/Library/LaunchAgents/com.wisent.skarbiec.plist"
    ));

    let extract = |unit: &str, key: &'static str| {
        let unit = unit.to_string();
        async move {
            let read = host_channel::run_program(
                resolved,
                &[
                    "/usr/bin/plutil",
                    "-extract",
                    &format!("EnvironmentVariables.{key}"),
                    "raw",
                    "-o",
                    "-",
                    unit.as_str(),
                ],
                runner,
            )
            .await
            .map_err(|error| error.to_string())?;
            Ok::<Option<String>, String>(if read.ok() {
                let declared = read.stdout.trim();
                (!declared.is_empty()).then(|| declared.to_string())
            } else {
                None
            })
        }
    };

    for unit in &units {
        let present = host_channel::remote_test(
            resolved,
            &format!("-f {}", crate::deploy::shlex_quote(unit)),
            runner,
        )
        .await
        .map_err(|error| error.to_string())?;
        if !present {
            continue;
        }
        // The unit that names a vault is the one serving this host; a unit that
        // does not is not a Skarbiec this read may borrow an environment from.
        let Some(vault) = extract(unit, "SKARBIEC_VAULT_FILE").await? else {
            continue;
        };
        let mut environment = vec![("SKARBIEC_VAULT_FILE".to_string(), vault)];
        if let Some(keyring) = extract(unit, "GNUPGHOME").await? {
            environment.push(("GNUPGHOME".to_string(), keyring));
        }
        return Ok(environment);
    }
    Err(
        "no managed Skarbiec unit on this host declares SKARBIEC_VAULT_FILE, so the live vault \
         cannot be identified and a read would silently answer from the wrong file"
            .to_string(),
    )
}

/// The public document, checked the way its consumers check it, before it is
/// allowed off the host.
fn judge_spis_trust(text: &str) -> Result<(), String> {
    let document: Value =
        serde_json::from_str(text).map_err(|_| "the renderer did not emit one JSON document")?;
    let fields = document
        .as_object()
        .ok_or("the rendered receipt trust is not a JSON object")?;
    if fields.len() != SPIS_TRUST_FIELDS.len()
        || !SPIS_TRUST_FIELDS
            .iter()
            .all(|name| fields.contains_key(*name))
    {
        return Err(format!(
            "the rendered receipt trust must carry exactly {}",
            SPIS_TRUST_FIELDS.join(", ")
        ));
    }
    if fields.get("schema").and_then(Value::as_str) != Some(SPIS_TRUST_SCHEMA) {
        return Err(format!(
            "the rendered receipt trust schema is not {SPIS_TRUST_SCHEMA}"
        ));
    }
    if fields.get("allowedAction").and_then(Value::as_str) != Some(SPIS_TRUST_ACTION) {
        return Err(format!(
            "the rendered allowedAction is not {SPIS_TRUST_ACTION}"
        ));
    }
    let organization = fields
        .get("organizationId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let uuid_shaped = organization.len() == 36
        && organization
            .chars()
            .enumerate()
            .all(|(index, character)| match index {
                8 | 13 | 18 | 23 => character == '-',
                _ => character.is_ascii_hexdigit(),
            });
    if !uuid_shaped {
        return Err("the rendered organizationId is not a UUID".to_string());
    }
    if fields
        .get("keySetVersion")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        return Err("the rendered keySetVersion is empty".to_string());
    }
    let keys = fields
        .get("receiptKeys")
        .and_then(Value::as_object)
        .ok_or("the rendered receiptKeys is not an object")?;
    if keys.is_empty() {
        return Err("the rendered receiptKeys is empty".to_string());
    }
    for (identifier, key) in keys {
        if identifier.trim().is_empty() {
            return Err("a rendered receipt key identifier is empty".to_string());
        }
        // The verifier hands this string straight to Node's Ed25519 `verify`,
        // which takes a PEM. A base64 body or a DER blob would be accepted
        // here and rejected at the first real receipt.
        match key.as_str() {
            Some(text) if text.contains("-----BEGIN PUBLIC KEY-----") => {}
            _ => return Err(format!("receipt key {identifier} is not a PEM public key")),
        }
    }
    // A private half in a document destined for a public repository is the one
    // mistake this command exists to make impossible.
    if text.contains("PRIVATE KEY") {
        return Err("the rendered document carries private key material".to_string());
    }
    Ok(())
}

/// `stado host render-spis-admission-trust TARGET SOURCE` — deliver the
/// checked-in Weles renderer to TARGET and print the public Spis receipt-trust
/// document it builds there.
///
/// The point of doing it this way is what does NOT travel. The admission
/// authority's private half stays in the vault it was minted into; the
/// renderer reads the vault on the host that holds it, assembles the
/// five-field public document, and only that document crosses the channel.
/// An operator station that renders locally would have to pull the item's
/// fields to itself first, and the four it needs are public only because the
/// fifth — which the same read would expose — is not.
///
/// Two audited halves and no third way in, the shape
/// [`sync_acquisition_scopes`] established: the renderer travels through the
/// [`stream_file`] delivery channel into `$HOME/.stado/files`, owner-only and
/// checksummed on arrival, and what runs is this command's own fixed argv.
/// Unlike that command this one reaps what it delivered — the retired helper
/// channel had a writer and no reaper, and `host provenance` still counts the
/// scripts it left behind.
pub async fn render_spis_admission_trust(target: &str, source: &str) -> Result<(), CmdError> {
    use crate::deploy::host_channel;

    let metadata = std::fs::symlink_metadata(source)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CmdError::usage("renderer source must be a regular file"));
    }
    let name = catalog_file_name(source)?;
    let (delivered, _bytes) = deliver_file(target, source, &name).await?;

    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let refused = |detail: String| {
        CmdError::click(format!(
            "{}: the renderer reached {delivered} and produced no document: {detail}",
            resolved.name
        ))
    };

    let home = host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    // `deliver_file` reports where the file landed for an operator to read —
    // with `$HOME` unexpanded, because that is the spelling the delivery
    // channel used. It is a message, not a path: quoting it for a remote test
    // asks about a directory literally named `$HOME`. The usable path is the
    // same one the channel built, composed here against the resolved home, the
    // way `register_acquisition_scopes` composes the catalog it reads.
    let installed = format!("{home}/{DELIVERED_FILES_DIR}/{name}");
    let declared = match live_skarbiec_environment(&resolved, &home, &runner).await {
        Ok(environment) => environment,
        Err(detail) => {
            remove_remote(&resolved, &[installed.as_str()], &runner).await;
            return Err(refused(detail));
        }
    };
    let vault = declared
        .iter()
        .find(|(key, _)| key == "SKARBIEC_VAULT_FILE")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    let skarbiec = format!("{home}/.stado/bin/skarbiec");

    // The renderer is a Node program, at the interpreter this fleet's macOS
    // hosts install; a host that resolves `node` elsewhere answers for itself
    // rather than being assumed.
    let brewed = "/opt/homebrew/bin/node";
    let node = if host_channel::remote_test(&resolved, &format!("-x {brewed}"), &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        brewed.to_string()
    } else {
        let looked_up = host_channel::run_command(&resolved, "command -v node", &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let found = looked_up.stdout.trim().to_string();
        if found.is_empty() {
            remove_remote(&resolved, &[installed.as_str()], &runner).await;
            return Err(refused(
                "no Node runtime is installed on this host".to_string(),
            ));
        }
        found
    };

    for file in [&skarbiec, &vault, &installed] {
        let present = host_channel::remote_test(
            &resolved,
            &format!("-f {}", crate::deploy::shlex_quote(file)),
            &runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        if !present {
            remove_remote(&resolved, &[installed.as_str()], &runner).await;
            return Err(refused(format!("required file is missing: {file}")));
        }
    }

    // The item id and the field names are the renderer's own compile-time
    // constants, so nothing that could name a secret field reaches this
    // command line, and no field VALUE ever does.
    //
    // The PATH is explicit for the same reason `register_acquisition_scopes`
    // sets one: Skarbiec decrypts by spawning GnuPG, and a login shell reached
    // through the channel does not necessarily carry the Homebrew prefix the
    // fleet installs it under.
    let mut assignments = vec![format!(
        "PATH={}",
        crate::deploy::shlex_quote(
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        )
    )];
    for (key, value) in &declared {
        assignments.push(format!("{key}={}", crate::deploy::shlex_quote(value)));
    }
    assignments.push(format!(
        "SKARBIEC_BIN={}",
        crate::deploy::shlex_quote(&skarbiec)
    ));
    let rendered = host_channel::run_command(
        &resolved,
        &format!(
            "{} {} {}",
            assignments.join(" "),
            crate::deploy::shlex_quote(&node),
            crate::deploy::shlex_quote(&installed),
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    remove_remote(&resolved, &[installed.as_str()], &runner).await;
    if !rendered.ok() {
        return Err(refused(host_channel::last_error_line(
            &rendered,
            "the renderer refused",
        )));
    }
    judge_spis_trust(&rendered.stdout).map_err(refused)?;

    // The host's own bytes, verbatim: this document is committed to a public
    // repository and compared byte-for-byte at activation, so re-serializing
    // it here would be this command quietly authoring it.
    print!("{}", rendered.stdout);
    if !rendered.stdout.ends_with('\n') {
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

/// One exact unmanaged executable, either inspected in place or moved into its
/// product-owned backup tree.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct RetireFileOutcome {
    pub target: String,
    pub source: String,
    pub destination: Option<String>,
    pub transaction: Option<String>,
    pub status: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub mode: Option<String>,
    pub detail: Option<String>,
}

impl RetireFileOutcome {
    fn succeeded(&self) -> bool {
        matches!(self.status.as_str(), "ready" | "retired" | "absent")
    }

    fn failure_sentence(&self) -> String {
        format!(
            "{}: {} {}{}",
            self.target,
            self.source,
            self.status,
            self.detail
                .as_ref()
                .map(|detail| format!(" — {detail}"))
                .unwrap_or_default()
        )
    }
}

fn retire_refused(message: impl Into<String>) -> CmdError {
    CmdError::click(format!("retire-file refused: {}", message.into()))
}

#[derive(Debug, Clone, Copy)]
pub struct RetireFileRequest<'a> {
    pub path: &'a str,
    pub product: &'a str,
    pub dry_run: bool,
    pub transaction: Option<&'a str>,
    pub expected_sha256: Option<&'a str>,
    pub expected_size: Option<u64>,
    pub expected_mode: Option<&'a str>,
}

#[derive(Debug)]
struct RetireFileBinding {
    transaction: String,
    expected_sha256: String,
    expected_size: u64,
    expected_mode: String,
}

fn safe_retirement_transaction(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 49
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'T'
        && bytes[9..15].iter().all(u8::is_ascii_digit)
        && bytes[15] == b'Z'
        && bytes[16] == b'-'
        && bytes[17..].iter().all(u8::is_ascii_hexdigit)
}

fn retire_file_binding(
    request: &RetireFileRequest<'_>,
) -> Result<Option<RetireFileBinding>, CmdError> {
    match (
        request.transaction,
        request.expected_sha256,
        request.expected_size,
        request.expected_mode,
    ) {
        (None, None, None, None) => Ok(None),
        (Some(transaction), Some(expected_sha256), Some(expected_size), Some(expected_mode)) => {
            if request.dry_run {
                return Err(CmdError::usage(
                    "preflight binding flags are accepted only by the mutating form",
                ));
            }
            if !safe_retirement_transaction(transaction) {
                return Err(CmdError::usage(
                    "transaction must be the exact token from a retire-file dry-run receipt",
                ));
            }
            if expected_sha256.len() != 64
                || !expected_sha256.as_bytes().iter().all(u8::is_ascii_hexdigit)
            {
                return Err(CmdError::usage(
                    "expected-sha256 must be a 64-digit hexadecimal SHA-256",
                ));
            }
            if expected_mode.len() != 4
                || !expected_mode
                    .as_bytes()
                    .iter()
                    .all(|byte| matches!(byte, b'0'..=b'7'))
            {
                return Err(CmdError::usage(
                    "expected-mode must be the four-digit octal mode from the dry-run receipt",
                ));
            }
            Ok(Some(RetireFileBinding {
                transaction: transaction.to_string(),
                expected_sha256: expected_sha256.to_ascii_lowercase(),
                expected_size,
                expected_mode: expected_mode.to_string(),
            }))
        }
        _ => Err(CmdError::usage(
            "transaction, expected-sha256, expected-size, and expected-mode must be supplied together",
        )),
    }
}

fn safe_backup_product(product: &str) -> bool {
    let bytes = product.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn c_path_component(component: &OsStr, label: &str) -> Result<CString, CmdError> {
    CString::new(component.as_bytes())
        .map_err(|_| retire_refused(format!("{label} contains a NUL byte")))
}

fn same_file(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn require_owned_directory(metadata: &Metadata, uid: u32, label: &str) -> Result<(), CmdError> {
    if !metadata.is_dir() {
        return Err(retire_refused(format!("{label} is not a directory")));
    }
    if metadata.uid() != uid {
        return Err(retire_refused(format!(
            "{label} is not owned by the approved account"
        )));
    }
    if (metadata.mode() & 0o022).ne(&0) {
        return Err(retire_refused(format!(
            "{label} is group- or world-writable"
        )));
    }
    Ok(())
}

fn open_home_directory(home: &Path, uid: u32) -> Result<File, CmdError> {
    let path_metadata = std::fs::symlink_metadata(home)
        .map_err(|error| retire_refused(format!("cannot inspect HOME: {error}")))?;
    if path_metadata.file_type().is_symlink() {
        return Err(retire_refused("HOME is a symlink"));
    }
    require_owned_directory(&path_metadata, uid, "HOME")?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open(home)
        .map_err(|error| retire_refused(format!("cannot open HOME: {error}")))?;
    let opened = directory
        .metadata()
        .map_err(|error| retire_refused(format!("cannot inspect opened HOME: {error}")))?;
    require_owned_directory(&opened, uid, "opened HOME")?;
    if !same_file(&path_metadata, &opened) {
        return Err(retire_refused("HOME changed while it was opened"));
    }
    Ok(directory)
}

fn open_directory_at(parent: RawFd, name: &OsStr, uid: u32) -> Result<Option<File>, CmdError> {
    let name = c_path_component(name, "directory name")?;
    let fd = unsafe {
        nix::libc::openat(
            parent,
            name.as_ptr(),
            nix::libc::O_RDONLY
                | nix::libc::O_DIRECTORY
                | nix::libc::O_NOFOLLOW
                | nix::libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(nix::libc::ENOENT) {
            return Ok(None);
        }
        return Err(retire_refused(format!(
            "cannot open directory component {:?}: {error}",
            name
        )));
    }
    let directory = unsafe { File::from_raw_fd(fd) };
    let metadata = directory
        .metadata()
        .map_err(|error| retire_refused(format!("cannot inspect directory: {error}")))?;
    require_owned_directory(&metadata, uid, "directory ancestor")?;
    Ok(Some(directory))
}

fn mkdir_at(parent: RawFd, name: &OsStr) -> Result<(), CmdError> {
    let name = c_path_component(name, "directory name")?;
    let result = unsafe { nix::libc::mkdirat(parent, name.as_ptr(), 0o700) };
    if result == 0 {
        Ok(())
    } else {
        Err(retire_refused(format!(
            "cannot create owner-only backup directory: {}",
            std::io::Error::last_os_error()
        )))
    }
}

fn open_or_create_directory_at(
    parent: RawFd,
    name: &OsStr,
    uid: u32,
    create: bool,
) -> Result<Option<File>, CmdError> {
    if let Some(directory) = open_directory_at(parent, name, uid)? {
        return Ok(Some(directory));
    }
    if !create {
        return Ok(None);
    }
    mkdir_at(parent, name)?;
    open_directory_at(parent, name, uid)?
        .map(Some)
        .ok_or_else(|| {
            retire_refused("backup directory disappeared immediately after it was created")
        })
}

fn open_source_at(parent: RawFd, name: &OsStr) -> Result<Option<File>, CmdError> {
    let name = c_path_component(name, "source basename")?;
    let fd = unsafe {
        nix::libc::openat(
            parent,
            name.as_ptr(),
            nix::libc::O_RDONLY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(nix::libc::ENOENT) {
            return Ok(None);
        }
        if error.raw_os_error() == Some(nix::libc::ELOOP) {
            return Err(retire_refused("source is a symlink"));
        }
        return Err(retire_refused(format!("cannot open source: {error}")));
    }
    Ok(Some(unsafe { File::from_raw_fd(fd) }))
}

fn entry_exists_at(parent: RawFd, name: &OsStr) -> Result<bool, CmdError> {
    let name = c_path_component(name, "path basename")?;
    let mut metadata = std::mem::MaybeUninit::<nix::libc::stat>::uninit();
    let result = unsafe {
        nix::libc::fstatat(
            parent,
            name.as_ptr(),
            metadata.as_mut_ptr(),
            nix::libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(nix::libc::ENOENT) {
        Ok(false)
    } else {
        Err(retire_refused(format!(
            "cannot inspect directory entry: {error}"
        )))
    }
}

fn remove_empty_directory_at(parent: RawFd, name: &OsStr) {
    if let Ok(name) = c_path_component(name, "transaction name") {
        unsafe {
            nix::libc::unlinkat(parent, name.as_ptr(), nix::libc::AT_REMOVEDIR);
        }
    }
}

#[cfg(target_os = "macos")]
fn rename_noreplace(
    source_parent: RawFd,
    source_name: &OsStr,
    destination_parent: RawFd,
    destination_name: &OsStr,
) -> std::io::Result<()> {
    let source_name = CString::new(source_name.as_bytes())?;
    let destination_name = CString::new(destination_name.as_bytes())?;
    let result = unsafe {
        nix::libc::renameatx_np(
            source_parent,
            source_name.as_ptr(),
            destination_parent,
            destination_name.as_ptr(),
            nix::libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn rename_noreplace(
    source_parent: RawFd,
    source_name: &OsStr,
    destination_parent: RawFd,
    destination_name: &OsStr,
) -> std::io::Result<()> {
    let source_name = CString::new(source_name.as_bytes())?;
    let destination_name = CString::new(destination_name.as_bytes())?;
    let result = unsafe {
        nix::libc::renameat2(
            source_parent,
            source_name.as_ptr(),
            destination_parent,
            destination_name.as_ptr(),
            nix::libc::RENAME_NOREPLACE as _,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn rename_noreplace(
    _source_parent: RawFd,
    _source_name: &OsStr,
    _destination_parent: RawFd,
    _destination_name: &OsStr,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

fn hash_open_file(file: &mut File) -> Result<String, CmdError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| retire_refused(format!("cannot seek file for hashing: {error}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| retire_refused(format!("cannot hash file: {error}")))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn source_unchanged(expected: &Metadata, observed: &Metadata) -> bool {
    same_file(expected, observed)
        && expected.uid() == observed.uid()
        && expected.mode() == observed.mode()
        && expected.len() == observed.len()
}

fn rollback_retirement(
    source_parent: RawFd,
    source_name: &OsStr,
    destination_parent: RawFd,
    destination_name: &OsStr,
) -> String {
    match entry_exists_at(source_parent, source_name) {
        Ok(true) => {
            "source path is still present; archived entry was retained for inspection".to_string()
        }
        Ok(false) => match rename_noreplace(
            destination_parent,
            destination_name,
            source_parent,
            source_name,
        ) {
            Ok(()) => "source was restored by atomic no-replace rename".to_string(),
            Err(error) => {
                format!("source restoration failed after the postcondition mismatch: {error}")
            }
        },
        Err(error) => format!("source restoration could not inspect the source path: {error}"),
    }
}

/// Run the device-local filesystem half of `host retire-file`.
///
/// Every path component is opened with `O_NOFOLLOW`, held by descriptor through
/// the mutation, and checked against the approved account uid. The source is
/// hashed through an open descriptor; the kernel rename is no-replace and
/// therefore cannot copy on `EXDEV` or overwrite a collision. The destination
/// must resolve to the same inode, size, mode, and digest. Any mismatch triggers
/// an atomic no-replace rollback before the command returns an error.
fn retire_file_local_document(
    request: &RetireFileRequest<'_>,
    binding: Option<&RetireFileBinding>,
) -> Result<RetireFileOutcome, CmdError> {
    let RetireFileRequest {
        path,
        product,
        dry_run,
        ..
    } = *request;
    if !path.starts_with('/')
        || path.split('/').any(|component| component == "..")
        || path.chars().any(char::is_control)
    {
        return Err(CmdError::usage(
            "path must be absolute, contain no '..' component, and carry no control character",
        ));
    }
    if !safe_backup_product(product) {
        return Err(CmdError::usage(
            "product must be 1-128 ASCII letters, digits, dots, underscores, or dashes and start with a letter or digit",
        ));
    }

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| retire_refused("HOME is not an absolute path"))?;
    let source = PathBuf::from(path);
    let approved = [
        (OsStr::new(".stado"), home.join(".stado/bin")),
        (OsStr::new(".local"), home.join(".local/bin")),
        (OsStr::new(".cargo"), home.join(".cargo/bin")),
    ];
    let (source_scope, _) = approved
        .iter()
        .find(|(_, root)| source.parent() == Some(root.as_path()))
        .ok_or_else(|| {
            retire_refused("source is not a direct child of an approved user bin root")
        })?;
    let source_name = source
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| retire_refused("source has no basename"))?;
    let uid = unsafe { nix::libc::geteuid() };
    let home_directory = open_home_directory(&home, uid)?;
    let Some(scope_directory) = open_directory_at(home_directory.as_raw_fd(), source_scope, uid)?
    else {
        return Ok(RetireFileOutcome {
            target: String::new(),
            source: path.to_string(),
            destination: None,
            transaction: None,
            status: "absent".to_string(),
            size: None,
            sha256: None,
            mode: None,
            detail: Some("approved source root does not exist".to_string()),
        });
    };
    let Some(source_directory) =
        open_directory_at(scope_directory.as_raw_fd(), OsStr::new("bin"), uid)?
    else {
        return Ok(RetireFileOutcome {
            target: String::new(),
            source: path.to_string(),
            destination: None,
            transaction: None,
            status: "absent".to_string(),
            size: None,
            sha256: None,
            mode: None,
            detail: Some("approved source root does not exist".to_string()),
        });
    };
    let Some(mut source_file) = open_source_at(source_directory.as_raw_fd(), source_name)? else {
        return Ok(RetireFileOutcome {
            target: String::new(),
            source: path.to_string(),
            destination: None,
            transaction: None,
            status: "absent".to_string(),
            size: None,
            sha256: None,
            mode: None,
            detail: Some("source does not exist".to_string()),
        });
    };
    let source_metadata = source_file
        .metadata()
        .map_err(|error| retire_refused(format!("cannot inspect source: {error}")))?;
    if !source_metadata.is_file() {
        return Err(retire_refused("source is not a regular file"));
    }
    if source_metadata.uid() != uid {
        return Err(retire_refused(
            "source is not owned by the approved account",
        ));
    }
    let size = source_metadata.len();
    let mode = format!("{:04o}", source_metadata.mode() & 0o7777);
    let sha256 = hash_open_file(&mut source_file)?;
    if let Some(binding) = binding {
        if size != binding.expected_size {
            return Err(retire_refused(
                "source size differs from the reviewed dry-run receipt",
            ));
        }
        if mode != binding.expected_mode {
            return Err(retire_refused(
                "source mode differs from the reviewed dry-run receipt",
            ));
        }
        if sha256 != binding.expected_sha256 {
            return Err(retire_refused(
                "source SHA-256 differs from the reviewed dry-run receipt",
            ));
        }
    }
    let transaction = binding
        .map(|binding| binding.transaction.clone())
        .unwrap_or_else(|| {
            format!(
                "{}-{}",
                chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
                uuid::Uuid::new_v4().simple()
            )
        });

    let destination_parts = [
        OsStr::new(".stado"),
        OsStr::new("products"),
        OsStr::new(product),
        OsStr::new("backups"),
    ];
    let mut destination_directories = Vec::<File>::with_capacity(destination_parts.len());
    let mut destination_parent_fd = home_directory.as_raw_fd();
    let mut destination_device = home_directory
        .metadata()
        .map_err(|error| retire_refused(format!("cannot inspect HOME: {error}")))?
        .dev();
    let mut missing_ancestor = false;
    for component in destination_parts {
        if missing_ancestor {
            continue;
        }
        match open_or_create_directory_at(destination_parent_fd, component, uid, !dry_run)? {
            Some(directory) => {
                destination_device = directory
                    .metadata()
                    .map_err(|error| {
                        retire_refused(format!("cannot inspect backup ancestor: {error}"))
                    })?
                    .dev();
                destination_directories.push(directory);
                destination_parent_fd = destination_directories
                    .last()
                    .expect("just pushed destination directory")
                    .as_raw_fd();
            }
            None => missing_ancestor = true,
        }
    }
    if source_metadata.dev() != destination_device {
        return Err(retire_refused(
            "source and backup tree are not on one filesystem, so an atomic move is impossible",
        ));
    }

    let destination = home
        .join(".stado/products")
        .join(product)
        .join("backups")
        .join(&transaction)
        .join(source_name);
    if dry_run {
        return Ok(RetireFileOutcome {
            target: String::new(),
            source: path.to_string(),
            destination: Some(destination.to_string_lossy().into_owned()),
            transaction: Some(transaction.clone()),
            status: "ready".to_string(),
            size: Some(size),
            sha256: Some(sha256),
            mode: Some(mode),
            detail: None,
        });
    }

    let backups_directory = destination_directories
        .last()
        .ok_or_else(|| retire_refused("backup tree was not created"))?;
    if source_metadata.dev().ne(&backups_directory
        .metadata()
        .map_err(|error| retire_refused(format!("cannot inspect backup root: {error}")))?
        .dev())
    {
        return Err(retire_refused(
            "source and backup tree are not on one filesystem, so an atomic move is impossible",
        ));
    }
    if entry_exists_at(backups_directory.as_raw_fd(), OsStr::new(&transaction))? {
        return Err(retire_refused("destination transaction collision"));
    }
    mkdir_at(backups_directory.as_raw_fd(), OsStr::new(&transaction))?;
    let transaction_directory =
        open_directory_at(backups_directory.as_raw_fd(), OsStr::new(&transaction), uid)?
            .ok_or_else(|| retire_refused("transaction directory disappeared after creation"))?;
    let transaction_metadata = transaction_directory.metadata().map_err(|error| {
        retire_refused(format!("cannot inspect transaction directory: {error}"))
    })?;
    if transaction_metadata.mode() & 0o077 != 0 {
        remove_empty_directory_at(backups_directory.as_raw_fd(), OsStr::new(&transaction));
        return Err(retire_refused("transaction directory is not owner-only"));
    }
    if entry_exists_at(transaction_directory.as_raw_fd(), source_name)? {
        remove_empty_directory_at(backups_directory.as_raw_fd(), OsStr::new(&transaction));
        return Err(retire_refused("destination collision"));
    }

    let current_source = open_source_at(source_directory.as_raw_fd(), source_name)?
        .ok_or_else(|| retire_refused("source disappeared before the move"))?;
    let current_metadata = current_source
        .metadata()
        .map_err(|error| retire_refused(format!("cannot re-inspect source: {error}")))?;
    if !source_unchanged(&source_metadata, &current_metadata) {
        remove_empty_directory_at(backups_directory.as_raw_fd(), OsStr::new(&transaction));
        return Err(retire_refused("source changed after it was observed"));
    }

    if let Err(error) = rename_noreplace(
        source_directory.as_raw_fd(),
        source_name,
        transaction_directory.as_raw_fd(),
        source_name,
    ) {
        remove_empty_directory_at(backups_directory.as_raw_fd(), OsStr::new(&transaction));
        return Err(retire_refused(format!(
            "atomic no-replace rename failed: {error}"
        )));
    }

    let postcondition = (|| -> Result<(), CmdError> {
        if entry_exists_at(source_directory.as_raw_fd(), source_name)? {
            return Err(retire_refused("source path was recreated during the move"));
        }
        let mut destination_file = open_source_at(transaction_directory.as_raw_fd(), source_name)?
            .ok_or_else(|| retire_refused("destination is absent after rename"))?;
        let destination_metadata = destination_file
            .metadata()
            .map_err(|error| retire_refused(format!("cannot inspect destination: {error}")))?;
        if !source_unchanged(&source_metadata, &destination_metadata) {
            return Err(retire_refused(
                "destination inode, owner, size, or mode differs from the opened source",
            ));
        }
        if hash_open_file(&mut destination_file)? != sha256 {
            return Err(retire_refused(
                "destination SHA-256 differs from the opened source",
            ));
        }
        Ok(())
    })();
    if let Err(error) = postcondition {
        let rollback = rollback_retirement(
            source_directory.as_raw_fd(),
            source_name,
            transaction_directory.as_raw_fd(),
            source_name,
        );
        return Err(CmdError::click(format!("{error}; {rollback}")));
    }

    Ok(RetireFileOutcome {
        target: String::new(),
        source: path.to_string(),
        destination: Some(destination.to_string_lossy().into_owned()),
        transaction: Some(transaction),
        status: "retired".to_string(),
        size: Some(size),
        sha256: Some(sha256),
        mode: Some(mode),
        detail: None,
    })
}

/// Hidden device-local endpoint used by the public target-resolving command.
pub fn retire_file_local(
    request: RetireFileRequest<'_>,
    json_output: bool,
) -> Result<(), CmdError> {
    let binding = retire_file_binding(&request)?;
    let outcome = retire_file_local_document(&request, binding.as_ref())?;
    if json_output {
        println!("{}", serde_json::to_string(&outcome)?);
    } else {
        print_retire_file_outcome(&outcome);
    }
    Ok(())
}

/// Resolve TARGET and invoke the same installed Rust primitive locally or over
/// its declared host channel. Remote cleanup therefore requires the 0.15.1
/// Stado delivery to be installed before any residue is touched.
async fn retire_file_document(
    target: &str,
    request: &RetireFileRequest<'_>,
    binding: Option<&RetireFileBinding>,
) -> Result<RetireFileOutcome, CmdError> {
    let RetireFileRequest {
        path,
        product,
        dry_run,
        ..
    } = *request;
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut outcome = if crate::deploy::host_channel::target_is_this_host(&resolved) {
        retire_file_local_document(request, binding)?
    } else {
        let runner = crate::deploy::production_runner();
        let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let binary = format!("{home}/.stado/bin/stado");
        let expected_size = binding.map(|binding| binding.expected_size.to_string());
        let mut words = vec![
            binary.as_str(),
            "host",
            "retire-file-local",
            path,
            "--product",
            product,
            "--json",
        ];
        if dry_run {
            words.push("--dry-run");
        }
        if let Some(binding) = binding {
            words.extend([
                "--transaction",
                binding.transaction.as_str(),
                "--expected-sha256",
                binding.expected_sha256.as_str(),
                "--expected-size",
                expected_size
                    .as_deref()
                    .expect("binding supplies expected size"),
                "--expected-mode",
                binding.expected_mode.as_str(),
            ]);
        }
        let output = crate::deploy::host_channel::run_program(&resolved, &words, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        if !output.ok() {
            return Err(CmdError::click(format!(
                "{}: installed Stado retire-file primitive failed: {}",
                resolved.name,
                crate::deploy::host_channel::last_error_line(
                    &output,
                    "remote command returned no detail"
                )
            )));
        }
        serde_json::from_str::<RetireFileOutcome>(output.stdout.trim()).map_err(|error| {
            CmdError::click(format!(
                "{}: installed Stado returned an invalid retirement report: {error}",
                resolved.name
            ))
        })?
    };
    outcome.target = resolved.name.clone();
    if outcome.succeeded() {
        Ok(outcome)
    } else {
        Err(CmdError::click(outcome.failure_sentence()))
    }
}

fn print_retire_file_outcome(outcome: &RetireFileOutcome) {
    if outcome.status == "absent" {
        println!("{}: {} absent", outcome.target, outcome.source);
    } else {
        println!(
            "{}: {} {} -> {} (transaction {}, {} bytes, sha256 {}, mode {})",
            outcome.target,
            outcome.source,
            outcome.status,
            outcome.destination.as_deref().unwrap_or("-"),
            outcome.transaction.as_deref().unwrap_or("-"),
            outcome.size.unwrap_or(0),
            outcome.sha256.as_deref().unwrap_or("-"),
            outcome.mode.as_deref().unwrap_or("-"),
        );
    }
}

/// `stado host retire-file TARGET PATH --product PRODUCT [--dry-run]`.
pub async fn retire_file(
    target: &str,
    request: RetireFileRequest<'_>,
    json_output: bool,
) -> Result<(), CmdError> {
    let binding = retire_file_binding(&request)?;
    let outcome = retire_file_document(target, &request, binding.as_ref()).await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        print_retire_file_outcome(&outcome);
    }
    Ok(())
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
# A service file is removable exactly where Stado installs service files.
# Keeping this list beside the delete is deliberate: a missing Linux path left
# retired user units on disk, ready for an older coordinator or a manual
# `systemctl enable` to resurrect. Root-owned machine units keep the same
# `com.wisent.*` namespace restriction as LaunchDaemons.
privileged=no
case "$path" in
  "$HOME/Library/LaunchAgents/"*|"$HOME/.stado/"*|"$HOME/.config/systemd/user/"*) ;;
  /Library/LaunchDaemons/com.wisent.*.plist|/etc/systemd/system/com.wisent.*.service) privileged=yes ;;
  *) report refused "outside the managed areas; remove it on the host with: sudo rm -- $path"; exit 0 ;;
esac
if [ -L "$path" ]; then
  report refused "a symlink points outside the managed area; remove it by hand: rm -- $path"
elif [ -d "$path" ]; then
  report refused "a directory is not removed by a single-file command"
elif [ ! -e "$path" ]; then
  report absent ""
elif [ ! -f "$path" ]; then
  report refused "not a regular file"
elif [ "$privileged" = yes ]; then
  # Owned by root by construction, so the `-O` test the home areas use would
  # refuse every one of them. The grant is the same `sudo -n` the install used;
  # a host without it is told which command was refused rather than left with a
  # unit nobody can remove.
  if /usr/bin/sudo -n /bin/rm -f -- "$path"; then
    if [ -e "$path" ]; then
      report failed "sudo rm succeeded and the path is still there"
    else
      report removed ""
    fi
  else
    report refused "sudo -n rm -- $path was refused; this host has no passwordless grant"
  fi
elif [ ! -O "$path" ]; then
  report refused "not owned by this account; remove it on the host with: sudo rm -- $path"
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

/// `stado host cron TARGET [--prune TEXT] [--apply] [--restore PATH]` — the
/// periodic table, and the one sanctioned way to change it.
///
/// A crontab is the last place on a fleet host where a process can be
/// declared outside both launchd and the registry, which is why every repair
/// this product makes to a unit can be undone by a reboot. See
/// [`crate::deploy::host_cron`] for the guards; they are on the host.
pub async fn cron(
    target: &str,
    prune: Option<&str>,
    restore: Option<&str>,
    apply: bool,
    json: bool,
) -> Result<(), CmdError> {
    if apply && prune.is_none() {
        return Err(CmdError::usage(
            "--apply changes a table, so it needs --prune to say which line",
        ));
    }
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let outcome = match restore {
        Some(path) => crate::deploy::host_cron::restore(&resolved, path, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?,
        None => crate::deploy::host_cron::prune(&resolved, prune.unwrap_or(""), apply, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&outcome.to_json())?);
    } else {
        println!("\n{}: crontab {}", outcome.host, outcome.state);
        if !outcome.detail.is_empty() {
            println!("  {}", outcome.detail);
        }
        if !outcome.table.is_empty() {
            println!("\nthe table as this host has it:");
            for row in &outcome.table {
                println!("  {row}");
            }
        }
        if !outcome.matched.is_empty() {
            println!("\nwhat the pattern reached:");
            for row in &outcome.matched {
                println!("  {row}");
            }
        }
        // Printed with the change, never left for the operator to compose.
        if let Some(command) = outcome.restore_command() {
            println!("\nthe table it replaced is saved; this puts it back:\n  {command}");
        }
    }
    if outcome.succeeded() {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "{}: crontab {} — {}",
            outcome.host, outcome.state, outcome.detail
        )))
    }
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

/// Per-field metadata of one decrypted item, computed on the host.
///
/// The value is what must not travel, and a length and a digest are not the
/// value: they are what lets a workstation prove that the item a declaration
/// references holds the bytes the operator has locally, without either side
/// sending them. So the decryption and the hashing both happen on the host,
/// and only this summary crosses the channel.
#[derive(serde::Deserialize)]
struct VaultFieldSummary {
    name: String,
    length: u64,
    sha256: String,
    text: bool,
}

#[derive(serde::Deserialize)]
struct VaultItemSummary {
    kind: Option<String>,
    schema: Option<String>,
    fields: Vec<VaultFieldSummary>,
}

/// The reducer that runs on the host: `skarbiec get` writes the decrypted
/// document to a pipe, and this reads it, replaces every value with its length
/// and SHA-256, and prints the summary. Nothing else is printed, so a value
/// cannot reach this process even by accident.
/// No indented block anywhere in it, deliberately: a Rust string literal that
/// continues with `\` drops the next line's leading whitespace, so an indented
/// `for` body arrives at the host as an `IndentationError`. A comprehension
/// needs no indentation and cannot lose it.
const VAULT_FIELD_SUMMARY_PROGRAM: &str = concat!(
    "import sys,json,hashlib\n",
    "document=json.load(sys.stdin)\n",
    "fields=document.get('fields') or {}\n",
    "encode=lambda value: (value if isinstance(value,str)",
    " else json.dumps(value,separators=(',',':'),sort_keys=True)).encode()\n",
    "print(json.dumps({'kind':document.get('kind'),'schema':document.get('schema'),",
    "'fields':[{'name':name,'text':isinstance(fields[name],str),",
    "'length':len(encode(fields[name])),",
    "'sha256':hashlib.sha256(encode(fields[name])).hexdigest()}",
    " for name in sorted(fields)]}))\n",
);

/// `stado host vault-item-show` — what one item on TARGET holds, without its
/// values.
///
/// `vault-item-put` had no counterpart, and the absence was not cosmetic: an
/// operator who had just written an item through the host channel could not
/// confirm from a workstation that the host held it, because
/// `retag-vault-item`'s read reports state, revision and tags and nothing
/// about the payload, `stado credentials get` reads the local store, and
/// `skarbiec get` is not a host-exec command. A migration wrote seven bundles
/// and twenty credential fields into a workstation vault that nothing on the
/// fleet reads, and the only reason it surfaced was a 401 from Brama.
///
/// So this reports the field NAMES with, per field, the value's length and its
/// SHA-256 — enough to compare against a local copy's digest and answer "does
/// the host hold what this row references", and never enough to learn the
/// value.
pub async fn vault_item_show(
    target: &str,
    item: &str,
    field: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("vault item", item)?;
    if let Some(field) = field {
        vault_word("field", field)?;
    }
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let environment = crate::deploy::host_channel::run_command(
        &resolved,
        "printf '%s\\n%s\\n' \"${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}\" \
         \"${GNUPGHOME:-$HOME/.gnupg}\"",
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    let refused = |detail: String| {
        CmdError::click(format!(
            "{}: {item} could not be read: {detail}",
            resolved.name
        ))
    };
    if !environment.ok() {
        return Err(refused(
            crate::deploy::host_channel::last_error_line(
                &environment,
                "the host's vault environment could not be read",
            )
            .to_string(),
        ));
    }
    let mut variables = environment.stdout.lines();
    let vault = variables.next().unwrap_or_default().to_string();
    let gnupg_home = variables.next().unwrap_or_default().to_string();
    let skarbiec = format!("{home}/.stado/bin/skarbiec");

    // The encrypted record first: an absent item is an answer, and it is the
    // answer that costs nothing to give.
    let record = read_vault_phase(&resolved, &vault, item, &runner)
        .await
        .map_err(refused)?;
    let updated_at = read_vault_updated_at(&resolved, &vault, item, &runner)
        .await
        .unwrap_or_else(|_| "-".to_string());
    if record.state == "absent" {
        if json_output {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "target": resolved.name,
                    "item": item,
                    "state": "absent",
                }))?
            );
        } else {
            println!("{}: {item} is absent", resolved.name);
        }
        return Ok(());
    }

    let summary_text = crate::deploy::host_channel::run_command(
        &resolved,
        &format!(
            "GNUPGHOME={} SKARBIEC_VAULT_FILE={} {} get {} --json | python3 -c {}",
            crate::deploy::shlex_quote(&gnupg_home),
            crate::deploy::shlex_quote(&vault),
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(item),
            crate::deploy::shlex_quote(VAULT_FIELD_SUMMARY_PROGRAM),
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !summary_text.ok() {
        // The last line of a remote failure is often the least informative one
        // - a decryption failure ends in a backtrace note - so the refusal
        // carries the host's own words, trimmed to what fits a terminal.
        let detail = summary_text
            .stderr
            .lines()
            .filter(|line| !line.trim().is_empty())
            .rev()
            .take(4)
            .collect::<Vec<&str>>()
            .into_iter()
            .rev()
            .collect::<Vec<&str>>()
            .join("; ");
        return Err(refused(if detail.is_empty() {
            "the host could not summarise the item's fields".to_string()
        } else {
            detail
        }));
    }
    let summary: VaultItemSummary = serde_json::from_str(summary_text.stdout.trim())
        .map_err(|error| refused(format!("the host's field summary did not parse: {error}")))?;
    let mut fields = summary.fields;
    if let Some(wanted) = field {
        fields.retain(|entry| entry.name == wanted);
        if fields.is_empty() {
            return Err(refused(format!("the item holds no field {wanted}")));
        }
    }

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "item": item,
                "state": record.state,
                "revision": record.revision,
                "tags": record.tags,
                "updated_at": updated_at,
                "kind": summary.kind,
                "schema": summary.schema,
                "fields": fields
                    .iter()
                    .map(|entry| json!({
                        "name": entry.name,
                        "length": entry.length,
                        "sha256": entry.sha256,
                        "text": entry.text,
                    }))
                    .collect::<Vec<Value>>(),
            }))?
        );
        return Ok(());
    }
    println!("host:       {}", resolved.name);
    println!("item:       {item}");
    println!("kind:       {}", summary.kind.as_deref().unwrap_or("-"));
    println!("schema:     {}", summary.schema.as_deref().unwrap_or("-"));
    println!("state:      {}", record.state);
    println!("revision:   {}", record.revision);
    println!("tags:       {}", record.tags);
    println!("updated_at: {updated_at}");
    for entry in &fields {
        println!(
            "field:      {} {} bytes sha256={}{}",
            entry.name,
            entry.length,
            entry.sha256,
            if entry.text { "" } else { " (structured)" }
        );
    }
    Ok(())
}

/// When the host last wrote this item. Read from the same encrypted record as
/// the revision, so it costs no decryption.
async fn read_vault_updated_at(
    resolved: &ComputeTarget,
    vault: &str,
    item: &str,
    runner: &crate::deploy::Runner,
) -> Result<String, String> {
    let text = crate::deploy::host_channel::remote_read_file(resolved, vault, runner)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("the vault at {vault} could not be read"))?;
    let document: Value = serde_json::from_str(&text)
        .map_err(|error| format!("the vault at {vault} did not parse as JSON: {error}"))?;
    Ok(document
        .get("items")
        .and_then(|items| items.get(item))
        .and_then(|record| record.get("updated_at"))
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_string())
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
    tags: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    vault_word("vault item", item)?;
    if let Some(tags) = tags {
        for tag in tags.split(',') {
            vault_word("tag", tag)?;
        }
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
    // No --tags: this is a read. Report what the host holds and write nothing,
    // so the operator who is about to replace a tag list can see the list they
    // would be replacing.
    let Some(tags) = tags else {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "target": resolved.name,
                    "item": item,
                    "read_only": true,
                    "state": before.state,
                    "revision": before.revision,
                    "tags": before.tags,
                }))?
            );
        } else {
            println!(
                "{}: {item} has rev={} state={} tags={}",
                resolved.name, before.revision, before.state, before.tags
            );
        }
        return Ok(());
    };
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
/// Store one canonical credential item in TARGET's owner vault.
///
/// The payload is accepted only on stdin and remains stdin across the host
/// channel. The command reports encrypted-record metadata before and after the
/// write; it never decrypts the value for reporting and never rewrites the
/// surrounding vault.
pub async fn vault_item_put(
    target: &str,
    item: &str,
    item_type: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("vault item", item)?;
    vault_word("credential type", item_type)?;

    let mut payload = String::new();
    std::io::stdin().lock().read_to_string(&mut payload)?;
    if payload.is_empty() || payload.len() > usize::from(u16::MAX) {
        return Err(CmdError::usage(
            "vault item payload must contain between one and 65535 bytes",
        ));
    }
    let document: Value = serde_json::from_str(&payload).map_err(|error| {
        CmdError::usage(format!("vault item payload is not valid JSON: {error}"))
    })?;
    let payload_type = document
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::usage("vault item payload requires a string kind"))?;
    if payload_type != item_type {
        return Err(CmdError::usage(format!(
            "vault item payload kind {payload_type:?} does not match --type {item_type:?}"
        )));
    }

    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
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
            "{}: the Skarbiec environment could not be read: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&environment, "remote command failed")
        )));
    }
    let mut variables = environment.stdout.lines();
    let vault = variables
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click(format!("{}: the vault path is empty", resolved.name)))?;
    let gnupg_home = variables
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click(format!("{}: GNUPGHOME is empty", resolved.name)))?;
    let skarbiec = format!("{home}/.stado/bin/skarbiec");
    let tool_path = skarbiec_tool_path(&home);
    let vault_environment = format!("SKARBIEC_VAULT_FILE={vault}");
    let gnupg_environment = format!("GNUPGHOME={gnupg_home}");
    let invocation = [
        "/usr/bin/env",
        tool_path.as_str(),
        gnupg_environment.as_str(),
        vault_environment.as_str(),
        skarbiec.as_str(),
        "set-json",
        item,
        "--type",
        item_type,
    ];

    let before = read_vault_phase(&resolved, vault, item, &runner)
        .await
        .map_err(CmdError::click)?;
    let stored = crate::deploy::host_channel::run_program_with_stdin(
        &resolved,
        &invocation,
        &payload,
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !stored.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec set-json failed for {item}: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&stored, "remote command failed")
        )));
    }
    let after = read_vault_phase(&resolved, vault, item, &runner)
        .await
        .map_err(CmdError::click)?;
    if after.state != "active" || after.revision == before.revision {
        return Err(CmdError::click(format!(
            "{}: {item} write was not visible in the encrypted vault",
            resolved.name
        )));
    }

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "item": item,
                "kind": item_type,
                "before": {
                    "state": before.state,
                    "revision": before.revision,
                },
                "after": {
                    "state": after.state,
                    "revision": after.revision,
                },
            }))?
        );
    } else {
        println!(
            "{}: stored {item} as {item_type}; state {} -> {}, revision {} -> {}",
            resolved.name, before.state, after.state, before.revision, after.revision
        );
    }
    Ok(())
}

/// Authorize one consumer to read one field of one item in TARGET's vault.
///
/// A Skarbiec grant is per item and per field, so widening what a unit or a
/// release job may read is a write into the *host's* vault, not into this
/// laptop's. The bearer never enters an argument vector: the consumer's
/// existing token file on the target is named, and Skarbiec reads it there.
pub async fn grant_item_read(
    target: &str,
    consumer: &str,
    item: &str,
    field: &str,
    token_file: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("consumer", consumer)?;
    vault_word("vault item", item)?;
    vault_word("item field", field)?;
    if token_file.trim().is_empty() {
        return Err(CmdError::usage(
            "--token-file must name the consumer's existing bearer file on the target",
        ));
    }

    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
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
            "{}: the Skarbiec environment could not be read: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&environment, "remote command failed")
        )));
    }
    let mut variables = environment.stdout.lines();
    let vault = variables
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click(format!("{}: the vault path is empty", resolved.name)))?;
    let gnupg_home = variables
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click(format!("{}: GNUPGHOME is empty", resolved.name)))?;
    let skarbiec = format!("{home}/.stado/bin/skarbiec");
    let tool_path = skarbiec_tool_path(&home);
    let vault_environment = format!("SKARBIEC_VAULT_FILE={vault}");
    let gnupg_environment = format!("GNUPGHOME={gnupg_home}");
    let bearer_path = if token_file.starts_with('/') {
        token_file.to_string()
    } else {
        format!("{home}/{}", token_file.trim_start_matches("~/"))
    };
    let invocation = [
        "/usr/bin/env",
        tool_path.as_str(),
        gnupg_environment.as_str(),
        vault_environment.as_str(),
        skarbiec.as_str(),
        "token-ensure-read",
        consumer,
        item,
        "--field",
        field,
        "--token-file",
        bearer_path.as_str(),
    ];
    let granted = crate::deploy::host_channel::run_program(&resolved, &invocation, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !granted.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec refused to grant {consumer} a read of {item}#{field}: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&granted, "remote command failed")
        )));
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "consumer": consumer,
                "item": item,
                "field": field,
                "token_file": bearer_path,
                "granted": true,
            }))?
        );
    } else {
        println!("{}: {consumer} may read {item}#{field}", resolved.name);
    }
    Ok(())
}

/// Report what one consumer's grant on TARGET actually holds.
///
/// Two facts decide every "not authorized to read item field" refusal, and
/// until now neither could be read: which capabilities the grant records, and
/// whether the bearer in the consumer's token file is the bearer the vault
/// recorded for it. Both are reported here as fields, so the answer to "may
/// this consumer read that" is a measurement rather than a failed attempt.
///
/// The bearer verdict is taken by re-asserting a capability the grant already
/// holds. Skarbiec's `token-ensure-read` compares the presented bearer first
/// and, for a capability already present, records nothing: it takes its
/// `unchanged` branch, writes no vault and appends no audit entry, and then
/// exercises the same two predicates the serving read applies. A grant with no
/// capability has nothing to re-assert, and says so instead of guessing.
pub async fn grant_show(
    target: &str,
    consumer: &str,
    token_file: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("consumer", consumer)?;
    let (resolved, listing) = remote_skarbiec_json(target, &[String::from("tokens")]).await?;
    let grant = listing
        .as_array()
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: Skarbiec did not answer its token list as an array",
                resolved.name
            ))
        })?
        .iter()
        .find(|entry| entry.get("consumer").and_then(Value::as_str) == Some(consumer));
    let Some(grant) = grant else {
        return Err(CmdError::click(format!(
            "{}: no grant is recorded for consumer {consumer}",
            resolved.name
        )));
    };
    let capabilities: Vec<String> = grant
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|capability| {
                    let item = capability
                        .get("item")
                        .and_then(Value::as_str)
                        .unwrap_or("<no item>");
                    let action = capability
                        .get("action")
                        .and_then(Value::as_str)
                        .unwrap_or("<no action>");
                    match capability.get("field").and_then(Value::as_str) {
                        Some(field) => format!("{item}#{field}:{action}"),
                        None => format!("{item}:{action}"),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let mut bearer_verdict = String::from("not checked: no --token-file was named");
    let mut effective: Option<bool> = None;
    if let Some(token_file) = token_file {
        // Arguments cross the host channel verbatim, with no shell to expand a
        // tilde, so the target's own home resolves the path here.
        let bearer_path = if token_file.starts_with('/') {
            token_file.to_string()
        } else {
            let runner = crate::deploy::production_runner();
            let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
            format!("{home}/{}", token_file.trim_start_matches("~/"))
        };
        let probe = grant
            .get("capabilities")
            .and_then(Value::as_array)
            .and_then(|entries| {
                entries.iter().find_map(|capability| {
                    if capability.get("action").and_then(Value::as_str) != Some("read") {
                        return None;
                    }
                    let item = capability.get("item").and_then(Value::as_str)?;
                    let field = capability.get("field").and_then(Value::as_str)?;
                    Some((item.to_string(), field.to_string()))
                })
            });
        match probe {
            None => {
                bearer_verdict = String::from(
                    "not checked: the grant records no field read to re-assert without widening it",
                );
            }
            Some((item, field)) => {
                let arguments = [
                    String::from("token-ensure-read"),
                    consumer.to_string(),
                    item,
                    String::from("--field"),
                    field,
                    String::from("--token-file"),
                    bearer_path.clone(),
                ];
                match remote_skarbiec_json(target, &arguments).await {
                    Ok((_, answer)) => {
                        effective = answer.get("effective").and_then(Value::as_bool);
                        bearer_verdict = match answer.get("refusal").and_then(Value::as_str) {
                            Some(reason) => format!("matches the recorded bearer; read {reason}"),
                            None => String::from("matches the recorded bearer"),
                        };
                    }
                    Err(error) => bearer_verdict = error.to_string(),
                }
            }
        }
    }

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "consumer": consumer,
                "audience": grant.get("audience"),
                "expires_at": grant.get("expires_at"),
                "workload_bound": grant.get("workload_bound"),
                "capabilities": capabilities,
                "token_file": token_file,
                "token_file_match": bearer_verdict,
                "effective": effective,
            }))?
        );
    } else {
        println!("{}: {consumer}", resolved.name);
        if capabilities.is_empty() {
            println!("  (the grant records no capability)");
        }
        for capability in &capabilities {
            println!("  {capability}");
        }
        println!("  token file: {bearer_verdict}");
    }
    Ok(())
}

fn skarbiec_tool_path(home: &str) -> String {
    format!(
        "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/local/MacGPG2/bin:{home}/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    )
}

/// Run one Skarbiec command on TARGET and parse its JSON answer.
///
/// The vault and GnuPG paths are resolved by the target itself. Arguments stay
/// separate all the way through the host channel, so neither bearer material
/// nor an operator-supplied capability enters a remote shell command.
async fn remote_skarbiec_json(
    target: &str,
    arguments: &[String],
) -> Result<(ComputeTarget, Value), CmdError> {
    remote_skarbiec_json_at(target, arguments, None).await
}

/// The mirror `skarbiec sync-pull` replaces the live vault from, relative to
/// the target account's home.
///
/// `sync_dir()` in Skarbiec's own `net::sync` reads `SKARBIEC_SYNC_DIR` and
/// otherwise takes `$HOME/.skarbiec-sync`, and the file inside it is always
/// `vault.enc.json`. Naming it here is what lets the preview list the mirror's
/// items with the same read-only `list` the live vault answers.
const SKARBIEC_MIRROR_RELATIVE: &str = ".skarbiec-sync/vault.enc.json";

/// [`remote_skarbiec_json`], optionally against a vault file other than the
/// target's live one.
///
/// The override exists for the sync preview and for nothing else: the only way
/// to say what a pull would change is to read the mirror as a vault, and
/// Skarbiec answers that question for whatever `SKARBIEC_VAULT_FILE` names.
/// The path is built from the target's own `$HOME`, never from an operator
/// argument, so this widens what Stado can read and not who can choose it.
async fn remote_skarbiec_json_at(
    target: &str,
    arguments: &[String],
    vault_relative: Option<&str>,
) -> Result<(ComputeTarget, Value), CmdError> {
    let command = arguments
        .first()
        .map(String::as_str)
        .ok_or_else(|| CmdError::usage("a Skarbiec command is required"))?;
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
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
            "{}: the Skarbiec environment could not be read: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&environment, "remote command failed")
        )));
    }
    let mut variables = environment.stdout.lines();
    let vault = variables
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click(format!("{}: the vault path is empty", resolved.name)))?;
    let gnupg_home = variables
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click(format!("{}: GNUPGHOME is empty", resolved.name)))?;
    let skarbiec = format!("{home}/.stado/bin/skarbiec");
    let vault_environment = match vault_relative {
        Some(relative) => format!("SKARBIEC_VAULT_FILE={home}/{relative}"),
        None => format!("SKARBIEC_VAULT_FILE={vault}"),
    };
    let gnupg_environment = format!("GNUPGHOME={gnupg_home}");
    let tool_path = skarbiec_tool_path(&home);
    let mut invocation = vec![
        "/usr/bin/env",
        tool_path.as_str(),
        gnupg_environment.as_str(),
        vault_environment.as_str(),
        skarbiec.as_str(),
    ];
    invocation.extend(arguments.iter().map(String::as_str));
    let output = crate::deploy::host_channel::run_program(&resolved, &invocation, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec {command} failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    let report = serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{}: Skarbiec {command} returned unreadable JSON: {error}",
            resolved.name
        ))
    })?;
    Ok((resolved, report))
}

/// One item as the target's own `skarbiec list` reports it, reduced to what a
/// sync verdict turns on.
struct MirrorItem {
    revision: i64,
    updated_at: String,
    deleted: bool,
}

fn mirror_items(
    report: &Value,
) -> Result<std::collections::BTreeMap<String, MirrorItem>, CmdError> {
    let rows = report
        .as_array()
        .ok_or_else(|| CmdError::click("Skarbiec list did not answer an array of items"))?;
    let mut items = std::collections::BTreeMap::new();
    for row in rows {
        let Some(id) = row.get("id").and_then(Value::as_str) else {
            continue;
        };
        items.insert(
            id.to_string(),
            MirrorItem {
                revision: row.get("revision").and_then(Value::as_i64).unwrap_or(-1),
                updated_at: row
                    .get("updated_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                deleted: row.get("deleted").and_then(Value::as_bool) == Some(true),
            },
        );
    }
    Ok(items)
}

/// What `stado host sync-vault` would do to TARGET, without doing any of it.
///
/// This preview exists because the operation it previews is not a merge, and
/// its name invites everyone to read it as one. `skarbiec sync-pull` copies the
/// mirror file over the live vault whole (`net::sync`: "A pull replaces the
/// whole live vault"), and merging is refused by design because "mirror and
/// live vault may be encrypted to different recipient sets". The only guard is
/// a refusal when a live item id is absent from the mirror; an id present on
/// both sides with different content is replaced by the mirror's copy with no
/// comment at all. So the interesting number is not how many items would be
/// added — it is how many would be replaced, and which of those something on
/// the host reads.
///
/// The comparison is `skarbiec list` against the live vault and against the
/// mirror file, both read-only, both over the same host channel. It reports the
/// mirror **as it currently sits on the host**: `sync-pull` runs `git pull`
/// first, so a mirror the target has not fetched yet can carry more than this
/// says. That is stated in the output rather than papered over, because a
/// preview that silently assumed a fetch would be a preview of a different
/// operation.
async fn preview_vault_sync(target: &str, json_output: bool) -> Result<(), CmdError> {
    let list = vec![String::from("list")];
    let (resolved, live_report) = remote_skarbiec_json(target, &list).await?;
    let (_, mirror_report) =
        remote_skarbiec_json_at(target, &list, Some(SKARBIEC_MIRROR_RELATIVE)).await?;
    let live = mirror_items(&live_report)?;
    let mirror = mirror_items(&mirror_report)?;

    let mut rows = Vec::new();
    let mut conflicts = 0usize;
    let mut lost = 0usize;
    let mut new = 0usize;
    let mut same = 0usize;
    for (id, mirrored) in &mirror {
        match live.get(id) {
            None => {
                new += 1;
                rows.push(json!({
                    "item": id,
                    "verdict": "new",
                    "mirror_revision": mirrored.revision,
                }));
            }
            Some(current)
                if current.revision == mirrored.revision
                    && current.updated_at == mirrored.updated_at =>
            {
                same += 1;
            }
            Some(current) => {
                conflicts += 1;
                rows.push(json!({
                    "item": id,
                    "verdict": "conflict",
                    "host_revision": current.revision,
                    "mirror_revision": mirrored.revision,
                    "host_updated_at": current.updated_at,
                    "mirror_updated_at": mirrored.updated_at,
                }));
            }
        }
    }
    // The same set Skarbiec's own `items_missing_from_mirror` computes, and for
    // the same reason it ignores tombstones: losing a soft-deleted item is not
    // losing data, and a pull that would drop a live one is refused outright.
    for (id, current) in &live {
        if current.deleted || mirror.contains_key(id) {
            continue;
        }
        lost += 1;
        rows.push(json!({
            "item": id,
            "verdict": "lost",
            "host_revision": current.revision,
        }));
    }

    let would_apply = lost == 0;
    let report = json!({
        "target": resolved.name,
        "mirror": format!("$HOME/{SKARBIEC_MIRROR_RELATIVE}"),
        "mirror_freshness": "as it sits on the host; sync-pull fetches first, so an unfetched mirror can carry more",
        "host_items": live.len(),
        "mirror_items": mirror.len(),
        "counts": {"new": new, "same": same, "conflict": conflicts, "lost": lost},
        "items": rows,
        "would_apply": would_apply,
        "detail": if would_apply {
            "sync-pull would replace the live vault file with the mirror; every shared item takes the mirror's copy"
        } else {
            "sync-pull would refuse: the live vault carries items the mirror does not"
        },
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{}: {} host item(s), {} mirror item(s) — {new} new, {same} same, {conflicts} conflict, {lost} lost",
            resolved.name,
            live.len(),
            mirror.len()
        );
        for row in &rows {
            println!(
                "  {:<8} {}",
                row["verdict"].as_str().unwrap_or_default(),
                row["item"].as_str().unwrap_or_default()
            );
        }
        println!("  {}", report["detail"].as_str().unwrap_or_default());
    }
    if conflicts == 0 && lost == 0 {
        Ok(())
    } else {
        Err(CmdError::silent(1))
    }
}

/// Pull the encrypted Skarbiec mirror into TARGET's live vault.
///
/// Not a merge, whatever the name suggests. `skarbiec sync-pull` copies the
/// mirror over the live vault whole; Skarbiec backs the live vault up first
/// and refuses when a live item id is absent from the mirror, and Stado
/// deliberately exposes no force path. An id on both sides takes the mirror's
/// copy with no comment, which is why `--check` exists and should be run
/// first: it names every item that would be replaced and every one that would
/// be lost.
pub async fn sync_vault(target: &str, check: bool, json_output: bool) -> Result<(), CmdError> {
    if check {
        return preview_vault_sync(target, json_output).await;
    }
    let (resolved, report) = remote_skarbiec_json(target, &[String::from("sync-pull")]).await?;
    if report.get("ok").and_then(Value::as_bool) != Some(true) {
        let reason = report
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("sync_refused");
        let detail = report
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("Skarbiec refused to replace the live vault");
        let local_only = report
            .get("local_only_items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",");
        let local_only = if local_only.is_empty() {
            String::new()
        } else {
            format!("; local-only items: {local_only}")
        };
        return Err(CmdError::click(format!(
            "{}: Skarbiec {reason}: {detail}{local_only}",
            resolved.name
        )));
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "status": "vault_synced",
                "skarbiec": report,
            }))?
        );
    } else {
        println!("{}: vault synced", resolved.name);
    }
    Ok(())
}

/// Mint a least-privilege bearer inside TARGET's live Skarbiec vault.
///
/// Metadata is the default output. `raw_token` exists only for a direct pipe
/// into another secret store; Stado never writes that bearer to disk or argv.
#[allow(clippy::too_many_arguments)]
pub async fn vault_token_mint(
    target: &str,
    consumer: &str,
    capabilities: &str,
    audience: &str,
    ttl_seconds: u64,
    replace_capabilities: bool,
    raw_token: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("consumer", consumer)?;
    vault_word("audience", audience)?;
    if raw_token && json_output {
        return Err(CmdError::usage(
            "--raw-token and --json cannot be used together",
        ));
    }
    if capabilities.is_empty()
        || !capabilities
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/,:#".contains(&byte))
    {
        return Err(CmdError::usage(
            "capabilities must be a comma-separated list of exact action:item[#field] values",
        ));
    }
    let mut arguments = vec![
        String::from("token-mint"),
        consumer.to_string(),
        String::from("--capabilities"),
        capabilities.to_string(),
        String::from("--audience"),
        audience.to_string(),
        String::from("--ttl-seconds"),
        ttl_seconds.to_string(),
    ];
    if replace_capabilities {
        arguments.push(String::from("--replace-capabilities"));
    }
    let (resolved, mut report) = remote_skarbiec_json(target, &arguments).await?;
    let token = report
        .get("token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: Skarbiec token-mint returned no bearer",
                resolved.name
            ))
        })?
        .to_string();
    if raw_token {
        println!("{token}");
        return Ok(());
    }
    if let Some(object) = report.as_object_mut() {
        object.remove("token");
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "status": "token_minted",
                "skarbiec": report,
            }))?
        );
    } else {
        println!(
            "{}: token minted for {consumer} with audience {audience}",
            resolved.name
        );
    }
    Ok(())
}

fn render_verifier_report(report: &Value, json_output: bool) -> Result<(), CmdError> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    let target = report.get("target").and_then(Value::as_str).unwrap_or("-");
    let consumer = report
        .get("consumer")
        .and_then(Value::as_str)
        .unwrap_or("-");
    let items = report
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(",");
    let verb = if report.get("exact").and_then(Value::as_bool) == Some(true) {
        "reads exactly"
    } else {
        "can read"
    };
    println!("{target}: {consumer} {verb} {items}");
    Ok(())
}

fn object_namespace_items(document: &Value) -> Result<BTreeMap<String, String>, CmdError> {
    let namespaces = document
        .pointer("/resolved/object_api_namespaces")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CmdError::click(
                "object_verifier_reconcile_host_declaration_unreadable: remote config has no resolved object_api_namespaces",
            )
        })?;
    namespaces
        .iter()
        .map(|(namespace, declaration)| {
            let item = declaration
                .get("item")
                .and_then(Value::as_str)
                .filter(|item| !item.is_empty())
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "object_verifier_reconcile_host_declaration_unreadable: namespace {namespace:?} has no item"
                    ))
                })?;
            Ok((namespace.clone(), item.to_string()))
        })
        .collect()
}

fn ensure_object_verifier_declarations_match(
    host: &BTreeMap<String, String>,
    local_items: &BTreeSet<String>,
) -> Result<(), CmdError> {
    let local = local_items
        .iter()
        .filter(|item| item.as_str() != crate::config::HOST_HEALTH_API_ITEM)
        .cloned()
        .collect::<BTreeSet<_>>();
    let host_items = host.values().cloned().collect::<BTreeSet<_>>();
    let missing = host
        .iter()
        .filter(|(_, item)| !local.contains(item.as_str()))
        .map(|(namespace, item)| format!("{namespace}={item}"))
        .collect::<Vec<_>>();
    let unexpected = local.difference(&host_items).cloned().collect::<Vec<_>>();
    if missing.is_empty() && unexpected.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "object_verifier_reconcile_declaration_mismatch: local object_api.namespaces cannot \
         prove TARGET's declaration (missing_local=[{}], unexpected_local_items=[{}]); copy \
         the host's exact namespace declarations locally before reconciling",
        missing.join(","),
        unexpected.join(",")
    )))
}

async fn reconcile_object_verifier_report(target: &str) -> Result<Value, CmdError> {
    let namespaces = crate::config::object_api_namespaces().map_err(|problems| {
        CmdError::click(format!(
            "invalid object_api.namespaces: {}",
            problems.join("; ")
        ))
    })?;
    let items = crate::config::object_verifier_items(namespaces);
    // Reconciliation used to derive "exact" solely from this machine's
    // declaration. On 2026-09-04 the target declared `spis-crawls`, this
    // machine did not, and the command removed nothing missing locally before
    // reporting exact=true while the target's whole object boundary stayed
    // closed. Read the configuration the target's services actually consume
    // and refuse before touching its grant when the two inputs differ.
    let canonical = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let stdout =
        remote_config_output(&canonical, None, &crate::deploy::production_runner()).await?;
    let document: Value = serde_json::from_str(&stdout).map_err(|error| {
        CmdError::click(format!(
            "object_verifier_reconcile_host_declaration_unreadable: {error}"
        ))
    })?;
    let host = object_namespace_items(&document)?;
    ensure_object_verifier_declarations_match(&host, &items)?;
    reconcile_verifier(
        target,
        "object",
        "matching local and target object_api.namespaces plus the host-health route",
        crate::config::OBJECT_API_VERIFIER_CONSUMER,
        "WC_OBJECT_SKARBIEC_TOKEN_FILE",
        "stado-object-api-verifier-skarbiec-token",
        items,
        true,
    )
    .await
}

/// Reconcile the dashboard verifier on TARGET to every configured object
/// namespace and the route-scoped host-health bearer.
pub async fn reconcile_object_verifier(target: &str, json_output: bool) -> Result<(), CmdError> {
    let report = reconcile_object_verifier_report(target).await?;
    render_verifier_report(&report, json_output)
}

/// Reconcile one product's release verifier dependency on TARGET.
pub async fn reconcile_release_verifier(
    target: &str,
    product: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let publishers = crate::config::release_api_publishers().map_err(|problems| {
        CmdError::click(format!(
            "invalid release_api.publishers: {}",
            problems.join("; ")
        ))
    })?;
    let publisher = publishers.get(product).ok_or_else(|| {
        CmdError::click(format!(
            "release_api.publishers declares no publisher for {product:?}"
        ))
    })?;
    let items = std::collections::BTreeSet::from([publisher.item().to_string()]);
    let report = reconcile_verifier(
        target,
        "release",
        &format!("release_api.publishers.{product}"),
        crate::config::RELEASE_API_VERIFIER_CONSUMER,
        "WC_RELEASE_SKARBIEC_TOKEN_FILE",
        "stado-release-api-verifier-skarbiec-token",
        items,
        false,
    )
    .await?;
    render_verifier_report(&report, json_output)
}

/// Reconcile the service verifier on TARGET to the exact configured deployer set.
pub async fn reconcile_service_verifier(target: &str, json_output: bool) -> Result<(), CmdError> {
    let deployers = crate::config::service_api_deployers().map_err(|problems| {
        CmdError::click(format!(
            "invalid service_api.deployers: {}",
            problems.join("; ")
        ))
    })?;
    let items = deployers
        .values()
        .map(|policy| policy.item().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    let report = reconcile_verifier(
        target,
        "service",
        "service_api.deployers",
        crate::config::SERVICE_API_VERIFIER_CONSUMER,
        "WC_SERVICE_SKARBIEC_TOKEN_FILE",
        "stado-service-api-verifier-skarbiec-token",
        items,
        true,
    )
    .await?;
    render_verifier_report(&report, json_output)
}

/// Read one nonsecret Skarbiec metadata report on a managed host.
///
/// Verifier reconciliation needs grant expiry, vault ownership and item lifecycle,
/// not the encrypted vault envelope. Reading those through Skarbiec keeps the
/// operator boundary intact and avoids transporting the whole vault over the host
/// channel.
async fn remote_skarbiec_metadata(
    target: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
    skarbiec: &str,
    vault: &str,
    gnupg_home: &str,
    command: &str,
) -> Result<Value, CmdError> {
    let path = "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin";
    let vault_environment = format!("SKARBIEC_VAULT_FILE={vault}");
    let gnupg_environment = format!("GNUPGHOME={gnupg_home}");
    let output = crate::deploy::host_channel::run_program(
        target,
        &[
            "/usr/bin/env",
            path,
            gnupg_environment.as_str(),
            vault_environment.as_str(),
            skarbiec,
            command,
        ],
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec {command} metadata unavailable: {}",
            target.name,
            crate::deploy::host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{}: Skarbiec {command} returned unreadable metadata: {error}",
            target.name
        ))
    })
}

/// Preserve an isolated verifier bearer while making its capabilities match config.
#[allow(clippy::too_many_arguments)]
async fn reconcile_verifier(
    target: &str,
    kind: &str,
    config_name: &str,
    consumer: &str,
    token_file_env: &str,
    token_file_default: &str,
    items: std::collections::BTreeSet<String>,
    replace_capabilities: bool,
) -> Result<Value, CmdError> {
    if items.is_empty() {
        return Err(CmdError::click(format!(
            "{config_name} is empty; refusing to mint an unusable verifier grant"
        )));
    }
    for item in &items {
        vault_word(&format!("{kind} verifier item"), item)?;
    }

    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let environment_command = format!(
        "printf '%s\\n%s\\n%s\\n' \"${{SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}}\" \
         \"${{GNUPGHOME:-$HOME/.gnupg}}\" \
         \"${{{token_file_env}:-$HOME/.stado/{token_file_default}}}\""
    );
    let environment =
        crate::deploy::host_channel::run_command(&resolved, &environment_command, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    if !environment.ok() {
        return Err(CmdError::click(format!(
            "{}: {kind} verifier environment could not be read: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&environment, "remote command failed")
        )));
    }
    let mut variables = environment.stdout.lines();
    let vault = variables.next().unwrap_or_default().to_string();
    let gnupg_home = variables.next().unwrap_or_default().to_string();
    let token_file = variables.next().unwrap_or_default().to_string();
    let skarbiec = format!("{home}/.stado/bin/skarbiec");
    for (label, path, test) in [
        ("Skarbiec binary", skarbiec.as_str(), "-x"),
        ("vault", vault.as_str(), "-f"),
    ] {
        let present = crate::deploy::host_channel::remote_test(
            &resolved,
            &format!("{test} {}", crate::deploy::shlex_quote(path)),
            &runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        if !present {
            return Err(CmdError::click(format!(
                "{}: no {label} at {path}",
                resolved.name
            )));
        }
    }
    let bearer_preserved = crate::deploy::host_channel::remote_test(
        &resolved,
        &format!(
            "-f {} && ! -L {}",
            crate::deploy::shlex_quote(&token_file),
            crate::deploy::shlex_quote(&token_file),
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;

    let token_metadata =
        remote_skarbiec_metadata(&resolved, &runner, &skarbiec, &vault, &gnupg_home, "tokens")
            .await?;
    let grant = token_metadata
        .as_array()
        .and_then(|tokens| {
            tokens
                .iter()
                .find(|entry| entry.get("consumer").and_then(Value::as_str) == Some(consumer))
        })
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: {consumer} has no existing grant",
                resolved.name
            ))
        })?;
    let expires_at = grant
        .get("expires_at")
        .and_then(Value::as_u64)
        .ok_or_else(|| CmdError::click(format!("{kind} verifier grant has no numeric expiry")))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| CmdError::click(error.to_string()))?
        .as_secs();
    let ttl = expires_at
        .checked_sub(now)
        .filter(|ttl| *ttl > 0)
        .ok_or_else(|| CmdError::click(format!("{kind} verifier grant is already expired")))?;
    // Release publisher items and the route-scoped host-health bearer remain
    // authoritative in the control-plane vault. Their consumers read
    // target-local shadows with the same ids. Atomically replace only those
    // shadows; this copies the current value without rotating or reclassifying
    // the authoritative source.
    let mut source_lifecycles = Vec::new();
    if matches!(kind, "release" | "object") {
        let authoritative_vault = crate::credential_store::owner::vault()
            .map_err(|error| CmdError::click(error.to_string()))?;
        let authoritative_text = std::fs::read_to_string(&authoritative_vault)?;
        let authoritative: Value = serde_json::from_str(&authoritative_text)?;
        let target_vaults =
            remote_skarbiec_metadata(&resolved, &runner, &skarbiec, &vault, &gnupg_home, "vaults")
                .await?;
        let target_owner = target_vaults
            .get("vaults")
            .and_then(Value::as_array)
            .and_then(|vaults| {
                vaults
                    .iter()
                    .find(|entry| entry.get("path").and_then(Value::as_str) == Some(vault.as_str()))
            })
            .and_then(|entry| entry.get("owner"))
            .and_then(Value::as_str)
            .ok_or_else(|| CmdError::click("target vault has no owner identity"))?;
        let target_items =
            remote_skarbiec_metadata(&resolved, &runner, &skarbiec, &vault, &gnupg_home, "list")
                .await?;
        for item in &items {
            if kind == "object" && item.as_str() != crate::config::HOST_HEALTH_API_ITEM {
                continue;
            }
            let source_entry = authoritative
                .get("items")
                .and_then(Value::as_object)
                .and_then(|entries| entries.get(item))
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "authoritative {kind} verifier source item {item} is absent"
                    ))
                })?;
            let source_management = source_entry
                .get("management")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "authoritative {kind} verifier source item {item} has no management metadata"
                    ))
                })?;
            let mode = source_management
                .get("mode")
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "owner" | "managed" | "external"))
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "authoritative {kind} verifier source item {item} has no supported lifecycle mode"
                    ))
                })?;
            let controller = source_management
                .get("controller")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "authoritative {kind} verifier source item {item} has no lifecycle controller"
                    ))
                })?;
            let token =
                crate::credential_store::owner::read_string(item, "token").map_err(|error| {
                    CmdError::click(format!(
                        "cannot read authoritative {kind} verifier source item {item}: {error}"
                    ))
                })?;
            let target_entry = target_items.as_array().and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.get("id").and_then(Value::as_str) == Some(item))
            });
            // Release shadows must be owned by the target vault. The
            // host-health item may itself be authoritative when the object API
            // runs beside the control-plane vault, so equality is sufficient.
            let shadow_owned = kind == "object"
                || target_entry
                    .and_then(|entry| entry.get("management"))
                    .and_then(Value::as_object)
                    .is_some_and(|management| {
                        management.get("mode").and_then(Value::as_str) == Some("owner")
                            && management.get("controller").and_then(Value::as_str)
                                == Some(target_owner)
                    });
            let compare_command = format!(
                "set -eu; expected=$(/bin/cat); actual=$(PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin GNUPGHOME={} SKARBIEC_VAULT_FILE={} {} get {} --field token); [ \"$actual\" = \"$expected\" ]",
                crate::deploy::shlex_quote(&gnupg_home),
                crate::deploy::shlex_quote(&vault),
                crate::deploy::shlex_quote(&skarbiec),
                crate::deploy::shlex_quote(item),
            );
            let mut comparison = crate::deploy::host_channel::run_program_with_stdin(
                &resolved,
                &["/bin/sh", "-c", &compare_command],
                &format!("{token}\n"),
                &runner,
            )
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
            if !shadow_owned || !comparison.ok() {
                let payload = serde_json::to_string(&serde_json::json!({
                    "schema": "skarbiec.item.v2",
                    "kind": "token",
                    "fields": { "token": token },
                    "context": {}
                }))?;
                let staging = format!("{vault}.stado-{kind}-verifier");
                let set_command = format!(
                    "set -eu; live={}; staging={}; trap '/bin/rm -f \"$staging\"' EXIT HUP INT TERM; /bin/cp -p \"$live\" \"$staging\"; PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin GNUPGHOME={} SKARBIEC_VAULT_FILE=\"$staging\" {} reclaim {} >/dev/null 2>&1 || true; PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin GNUPGHOME={} SKARBIEC_VAULT_FILE=\"$staging\" {} rm {} >/dev/null 2>&1 || true; PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin GNUPGHOME={} SKARBIEC_VAULT_FILE=\"$staging\" {} set-json {} --type token >/dev/null; /bin/chmod 600 \"$staging\"; /bin/mv -f \"$staging\" \"$live\"; trap - EXIT HUP INT TERM",
                    crate::deploy::shlex_quote(&vault),
                    crate::deploy::shlex_quote(&staging),
                    crate::deploy::shlex_quote(&gnupg_home),
                    crate::deploy::shlex_quote(&skarbiec),
                    crate::deploy::shlex_quote(item),
                    crate::deploy::shlex_quote(&gnupg_home),
                    crate::deploy::shlex_quote(&skarbiec),
                    crate::deploy::shlex_quote(item),
                    crate::deploy::shlex_quote(&gnupg_home),
                    crate::deploy::shlex_quote(&skarbiec),
                    crate::deploy::shlex_quote(item),
                );
                let convergence_transport_error =
                    match crate::deploy::host_channel::run_program_with_stdin(
                        &resolved,
                        &["/bin/sh", "-c", &set_command],
                        &payload,
                        &runner,
                    )
                    .await
                    {
                        Ok(converged) if converged.ok() => None,
                        // A vault replacement can close the host channel after
                        // the atomic move but before the shell reports success.
                        // A nonzero transport-shaped result is therefore as
                        // ambiguous as an I/O error: reconnect and prove the
                        // target item before deciding whether the write failed.
                        Ok(converged) => Some(
                            crate::deploy::host_channel::last_error_line(
                                &converged,
                                "verifier shadow command ended before acknowledgement",
                            )
                            .to_string(),
                        ),
                        // Replacing the vault may invalidate the transport whose
                        // credential came from that vault. Reconnect and judge the
                        // item by its postcondition instead of repeating the write.
                        Err(error) => Some(error.to_string()),
                    };
                comparison = crate::deploy::host_channel::run_program_with_stdin(
                    &resolved,
                    &["/bin/sh", "-c", &compare_command],
                    &format!("{token}\n"),
                    &runner,
                )
                .await
                .map_err(|error| {
                    let first = convergence_transport_error
                        .as_deref()
                        .unwrap_or("shadow write completed");
                    CmdError::click(format!(
                        "{}: cannot verify {kind} verifier shadow for {item} after {first}: {error}",
                        resolved.name
                    ))
                })?;
            }
            if !comparison.ok() {
                return Err(CmdError::click(format!(
                    "{}: {kind} verifier shadow for {item} differs after reconciliation",
                    resolved.name,
                )));
            }
            source_lifecycles.push(json!({
                "item": item,
                "mode": mode,
                "controller": controller,
                "readable": true,
            }));
        }
    }
    let capabilities = items
        .iter()
        .map(|item| format!("read:{item}#token"))
        .collect::<Vec<_>>()
        .join(",");
    let common = format!(
        "set -eu; \
         PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin; export PATH; \
         GNUPGHOME={}; export GNUPGHOME; \
         SKARBIEC_VAULT_FILE={}; export SKARBIEC_VAULT_FILE; \
         token_file={}; staged=''; \
         if [ -L \"$token_file\" ]; then exit 40; fi",
        crate::deploy::shlex_quote(&gnupg_home),
        crate::deploy::shlex_quote(&vault),
        crate::deploy::shlex_quote(&token_file),
    );
    let command = if replace_capabilities {
        format!(
            "{common}; \
             if [ -f \"$token_file\" ]; then source_file=\"$token_file\"; \
             else \
               staged=\"$token_file.stado-new.$$\"; \
               trap '/bin/rm -f \"$staged\"' EXIT HUP INT TERM; \
               umask 077; /usr/bin/openssl rand -hex 32 > \"$staged\"; \
               source_file=\"$staged\"; \
             fi; \
             {} token-mint {} --capabilities {} --replace-capabilities \
               --token-file \"$source_file\" --ttl-seconds {} > /dev/null; \
             if [ -n \"$staged\" ]; then /bin/mv -f \"$staged\" \"$token_file\"; trap - EXIT HUP INT TERM; fi",
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(consumer),
            crate::deploy::shlex_quote(&capabilities),
            ttl,
        )
    } else {
        let item = items
            .first()
            .expect("product-scoped release verifier has one item");
        format!(
            "{common}; \
             if [ -f \"$token_file\" ]; then \
               {} token-ensure-read {} {} --field token --token-file \"$token_file\" > /dev/null; \
             else \
               staged=\"$token_file.stado-new.$$\"; \
               trap '/bin/rm -f \"$staged\"' EXIT HUP INT TERM; \
               umask 077; /usr/bin/openssl rand -hex 32 > \"$staged\"; \
               {} token-mint {} --capabilities {} --replace-capabilities \
                 --token-file \"$staged\" --ttl-seconds {} > /dev/null; \
               /bin/mv -f \"$staged\" \"$token_file\"; trap - EXIT HUP INT TERM; \
             fi",
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(consumer),
            crate::deploy::shlex_quote(item),
            crate::deploy::shlex_quote(&skarbiec),
            crate::deploy::shlex_quote(consumer),
            crate::deploy::shlex_quote(&capabilities),
            ttl,
        )
    };
    let reconciled = crate::deploy::host_channel::run_command(&resolved, &command, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !reconciled.ok() {
        return Err(CmdError::click(format!(
            "{}: {kind} verifier reconciliation failed without replacing its token file: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&reconciled, "remote command failed")
        )));
    }

    let item_list = items.iter().cloned().collect::<Vec<_>>();
    let report = json!({
        "target": resolved.name,
        "kind": kind,
        "consumer": consumer,
        "items": item_list,
        "source_lifecycles": source_lifecycles,
        "bearer_preserved": bearer_preserved,
        "expires_at": expires_at,
        "exact": replace_capabilities,
    });
    Ok(report)
}

/// Recover a Skarbiec audit-lock stall on TARGET and restart only its loaded
/// dependants.
///
/// The helper runs on TARGET, so the endpoints it probes must be TARGET's. Its
/// own defaults are this fleet's operator laptop -- Skarbiec on 8787, the
/// object API on 18765 -- and on 2026-09-03 that refused a real audit-lock
/// stall on `charless-mac-mini`, where Skarbiec answers 8895 and the object API
/// 8765, with "did not report an audit-lock failure": the script had asked a
/// port nothing serves on that host and read the silence as health. The
/// registry already states where each service answers per asking machine, so
/// resolve TARGET's own endpoints and hand them over rather than letting a
/// hard-coded default decide which host the operator meant.
pub async fn recover_skarbiec_audit(target: &str, json_output: bool) -> Result<(), CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let probes = target_health_probes(&resolved.name).await;
    let script = format!(
        "{probes}{}",
        include_str!("../../../scripts/recover-skarbiec-audit-lock.sh")
    );
    let recovered = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        &script,
        std::time::Duration::from_secs(90),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !recovered.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec audit recovery failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&recovered, "remote command failed")
        )));
    }
    let detail = recovered.stdout.trim();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "recovered": detail.contains("recovered"),
                "detail": detail,
            }))?
        );
    } else {
        println!("{}: {detail}", resolved.name);
    }
    Ok(())
}

/// Shell prologue exporting the health endpoints TARGET itself serves on, read
/// from the registry's service directory.
///
/// The directory keys every endpoint by the asking machine because these
/// services bind loopback on their own host, so "where is Skarbiec" has a
/// different true answer on every machine. A helper that runs ON the target is
/// the target asking, which is why the lookup is `address_for(target)` and not
/// this laptop's own row.
///
/// Missing rows export nothing and leave the script's defaults alone: a
/// recovery that cannot name the endpoint should refuse on the endpoint it
/// documents rather than on one this function invented.
async fn target_health_probes(target: &str) -> String {
    let Ok(registry) = super::registry::read_registry().await else {
        return String::new();
    };
    let Some(directory) = registry.service_directory.as_ref() else {
        return String::new();
    };
    let mut prologue = String::new();
    for (service, variable, path) in [
        ("skarbiec", "SKARBIEC_HEALTH_URL", "/health"),
        ("stado-object-api", "STADO_OBJECT_HEALTH_URL", "/healthz"),
    ] {
        let Some(url) = directory
            .services
            .get(service)
            .and_then(|entry| entry.address_for(target))
            .map(|endpoint| endpoint.url.trim_end_matches('/').to_string())
        else {
            continue;
        };
        prologue.push_str(&format!(
            "{variable}=${{{variable}:-{}}}\nexport {variable}\n",
            crate::deploy::shlex_quote(&format!("{url}{path}"))
        ));
    }
    prologue
}

/// Recover stale per-user GnuPG daemons after Skarbiec reports a keybox stall.
pub async fn recover_skarbiec_crypto(target: &str, json_output: bool) -> Result<(), CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let recovered = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        include_str!("../../../scripts/recover-skarbiec-crypto.sh"),
        std::time::Duration::from_secs(240),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !recovered.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec cryptographic recovery failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&recovered, "remote command failed")
        )));
    }
    let detail = recovered.stdout.trim();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "recovered": detail.contains("recovered"),
                "detail": detail,
            }))?
        );
    } else {
        println!("{}: {detail}", resolved.name);
    }
    Ok(())
}

/// Repair Skarbiec's short-lived acquisition state after a service-user cutover.
pub async fn recover_skarbiec_acquisition_state(
    target: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let recovered = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        include_str!("../../../scripts/recover-skarbiec-acquisition-state.sh"),
        std::time::Duration::from_secs(90),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !recovered.ok() {
        return Err(CmdError::click(format!(
            "{}: Skarbiec acquisition-state recovery failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&recovered, "remote command failed")
        )));
    }
    let detail = recovered.stdout.trim();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "recovered": detail.contains("recovered"),
                "detail": detail,
            }))?
        );
    } else {
        println!("{}: {detail}", resolved.name);
    }
    Ok(())
}

/// Restore the core object API without depending on the API being available.
///
/// The fixed-script channel transports the checked-in helper verbatim. Its
/// only prerequisite mutation is the helper's exact-owned orphaned Skarbiec
/// proxy reconciliation; object recovery itself mutates only a listener whose
/// authenticated protected read is unavailable.
async fn recover_object_api_on_target(
    resolved: &ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<String, CmdError> {
    let recovered = crate::deploy::host_channel::run_script_with_timeout(
        resolved,
        include_str!("../../../deploy/recover_object_api.sh"),
        std::time::Duration::from_secs(240),
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !recovered.ok() {
        return Err(CmdError::click(format!(
            "{}: object API recovery failed: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&recovered, "remote command failed")
        )));
    }
    Ok(recovered.stdout.trim().to_string())
}

/// Run the object-API boundary repair as a focused operator command.
pub async fn recover_object_api(target: &str, json_output: bool) -> Result<(), CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let detail = recover_object_api_on_target(&resolved, &runner).await?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "healthy": true,
                "detail": detail,
            }))?
        );
    } else {
        println!("{}: {detail}", resolved.name);
    }
    Ok(())
}

/// Authorize TARGET's service resolver on the service-directory authority.
pub async fn authorize_resolver_key(target: &str, json_output: bool) -> Result<(), CmdError> {
    let report = crate::deploy::host_resolver_key::authorize(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{}: resolver key {} ({}), authorized_keys on {} {}",
            report["target"].as_str().unwrap_or_default(),
            report["key_state"].as_str().unwrap_or_default(),
            report["key_type"].as_str().unwrap_or_default(),
            report["authority"].as_str().unwrap_or_default(),
            report["authorized_keys"].as_str().unwrap_or_default(),
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

/// One collected managed-unit log, before the CLI or a higher-level
/// diagnostic chooses how to render it.
struct UnitLogReport {
    target: String,
    unit: String,
    lines: u32,
    declared: Vec<Value>,
    log: String,
}

impl UnitLogReport {
    fn to_json(&self) -> Value {
        json!({
            "target": self.target,
            "unit": self.unit,
            "lines": self.lines,
            "declared": self.declared,
            "log": self.log,
        })
    }
}

/// Collect a managed unit's declared logs without rendering them.
///
/// `host unit-log` and higher-level diagnostics share this collector so the
/// first one never knows a failure sentence the second one cannot see.
async fn collect_unit_log(
    resolved: &ComputeTarget,
    unit: &str,
    lines: u32,
    runner: &crate::deploy::Runner,
) -> Result<UnitLogReport, CmdError> {
    let home = crate::deploy::host_channel::remote_home(resolved, runner)
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
            resolved,
            &format!("-f {}", crate::deploy::shlex_quote(&candidate)),
            runner,
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

    let mut sources = vec![json!({"kind": "plist", "path": plist})];
    let mut body = String::new();

    // One reader for both keys: a unit that sends stdout and stderr to the
    // same file must not be tailed twice, and a unit that separates them must
    // not have half of its account silently dropped.
    let out_path = unit_log_path(resolved, "Print :StandardOutPath", &plist, runner).await?;
    let err_path = unit_log_path(resolved, "Print :StandardErrorPath", &plist, runner).await?;
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
            resolved,
            &format!("-f {}", crate::deploy::shlex_quote(log)),
            runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        {
            sources.push(json!({"kind": "file", "path": log}));
            body.push_str(&format!("=== {log} (last {lines} lines)\n"));
            let tail = crate::deploy::host_channel::run_program(
                resolved,
                &["/usr/bin/tail", "-n", &lines.to_string(), "--", log],
                runner,
            )
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
            if tail.ok() {
                body.push_str(&tail.stdout);
            } else {
                body.push_str("    unreadable\n");
            }
        } else {
            sources.push(json!({"kind": "absent", "path": log}));
            body.push_str(&format!("=== {log} (absent)\n"));
        }
    }

    Ok(UnitLogReport {
        target: resolved.name.clone(),
        unit: unit.to_string(),
        lines,
        declared: sources,
        log: body.trim_end().to_string(),
    })
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
    let report = collect_unit_log(&resolved, unit, lines, &runner).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report.to_json())?);
    } else {
        println!("{}", report.log);
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
const legacyWorkerRoot = path.join(home, '.local/share/weles-worker');
const managedServiceRoot = path.join(home, '.stado/services/weles-admission');
const managedWorkerRoot = path.join(managedServiceRoot, 'current');

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

const compareVersions = (left, right) => {
  const parts = (value) => String(value).split('.').map((piece) => Number.parseInt(piece, 10) || 0);
  const [a, b] = [parts(left), parts(right)];
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
};

const releaseVersions = new Set();
const recordingSources = [];
const addRecordingSource = (release, platform, recordings, priority) => {
  if (typeof release !== 'string' || !release) return;
  try {
    if (!fs.statSync(recordings).isDirectory()) return;
  } catch {
    return;
  }
  releaseVersions.add(release);
  recordingSources.push({ release, platform, recordings, priority });
};
const addManagedRuntime = (runtime, platform, priority) => {
  const manifest = readJson(path.join(runtime, 'package.json'));
  const release = typeof manifest?.version === 'string' && manifest.version
    ? manifest.version
    : null;
  if (release) releaseVersions.add(release);
  addRecordingSource(release, platform, path.join(runtime, 'recordings'), priority);
};

// `current` is the active immutable coordinate. Count its release even before
// the first browser run creates a recordings directory.
addManagedRuntime(path.join(managedWorkerRoot, 'runtime'), 'managed', 2);

// Also report every immutable release Stado installed. The service store is
// digest-addressed (`sha256-*/<platform>/runtime`), not version-addressed, and
// tying release discovery to a recordings directory hid fresh installations
// until their first browser artifact existed.
try {
  for (const releaseEntry of fs.readdirSync(managedServiceRoot, { withFileTypes: true })) {
    if (!releaseEntry.isDirectory() || !releaseEntry.name.startsWith('sha256-')) continue;
    const releaseRoot = path.join(managedServiceRoot, releaseEntry.name);
    for (const platformEntry of fs.readdirSync(releaseRoot, { withFileTypes: true })) {
      if (!platformEntry.isDirectory()) continue;
      addManagedRuntime(
        path.join(releaseRoot, platformEntry.name, 'runtime'),
        platformEntry.name,
        1,
      );
    }
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}

// Keep reporting recordings written by the retired per-version installer while
// hosts complete their cutover to the fleet-managed service.
try {
  for (const releaseEntry of fs.readdirSync(legacyWorkerRoot, { withFileTypes: true })) {
    if (!releaseEntry.isDirectory()) continue;
    const release = releaseEntry.name;
    const releaseRoot = path.join(legacyWorkerRoot, release);
    for (const platformEntry of fs.readdirSync(releaseRoot, { withFileTypes: true })) {
      if (!platformEntry.isDirectory()) continue;
      addRecordingSource(
        release,
        platformEntry.name,
        path.join(releaseRoot, platformEntry.name, 'recordings'),
        0,
      );
    }
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}
const releases = [...releaseVersions].sort(compareVersions);

// The version marker names the release the retired activator staged. It can
// disagree with the active fleet-managed release and remains useful evidence
// that the old delivery path has not been removed from a host yet.
const releaseMarker = (() => {
  try {
    return fs.readFileSync(path.join(home, '.stado/files/weles-release-version'), 'utf8').trim() || null;
  } catch {
    return null;
  }
})();

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
  let resultHealthy = null;
  let resultSignal = null;
  let resultAt = null;
  let uploadProof = null;
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
      } else if (entry.name === 'ban_signal.json') {
        const document = readJson(full);
        if (typeof document?.healthy === 'boolean') resultHealthy = document.healthy;
        if (typeof document?.signal === 'string' && document.signal) resultSignal = document.signal;
        if (typeof document?.ts === 'string') resultAt = document.ts;
      } else if (entry.name === '.uploaded.json') {
        const document = readJson(full);
        if (typeof document?.sha256 === 'string' && typeof document?.destination === 'string') {
          uploadProof = { sha256: document.sha256, destination: document.destination };
        }
      } else if (entry.name === 'session_meta.json') {
        const document = readJson(full);
        if (typeof document?.started_at === 'string') startedAt = document.started_at;
      }
    }
  };
  walk(runDirectory, 0);

  if (!uploadProof) {
    const document = readJson(path.join(runDirectory, '.uploaded.json'));
    if (typeof document?.sha256 === 'string' && typeof document?.destination === 'string') {
      uploadProof = { sha256: document.sha256, destination: document.destination };
    }
  }
  const costs = readJson(path.join(path.dirname(runDirectory), '_costs', `${path.basename(runDirectory)}.json`));
  const isFresh = Date.now() - stat.mtimeMs < RUNNING_WINDOW_MS;

  let status = 'recorded';
  if (resultHealthy === true || resultOk === true) status = 'succeeded';
  else if (resultHealthy === false || resultOk === false || resultSignal) status = 'failed';
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
    result: resultHealthy !== null || resultSignal
      ? { healthy: resultHealthy, signal: resultSignal, recorded_at: resultAt }
      : null,
    uploaded: uploadProof !== null,
    upload_proof: uploadProof,
  };
};

const runsById = new Map();
for (const source of recordingSources) {
  let entries = [];
  try {
    entries = fs.readdirSync(source.recordings, { withFileTypes: true });
  } catch {
    continue;
  }
  for (const entry of entries) {
    // `_costs` is the sidecar ledger of the runs beside it, not a run.
    if (!entry.isDirectory() || entry.name === '_costs') continue;
    const candidate = {
      release: source.release,
      platform: source.platform,
      directory: path.join(source.recordings, entry.name),
      priority: source.priority,
    };
    const existing = runsById.get(entry.name);
    if (!existing || candidate.priority > existing.priority) runsById.set(entry.name, candidate);
  }
}
const runs = [...runsById.values()];
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

const describedById = new Map();
for (const row of runs) {
  const summary = describeRun(row.release, row.platform, row.directory);
  describedById.set(summary.id, summary);
}

// Weles API requests keep their process result outside a release runtime so an
// update cannot erase it. Fold those durable records into the live recording
// inventory: a cleaned recording loses its artifact counts, not the fact that
// the run happened or how its process ended.
const detachedRoot = path.join(home, '.stado/weles-detached-runs');
try {
  for (const entry of fs.readdirSync(detachedRoot, { withFileTypes: true })) {
    if (!entry.isFile() || !entry.name.endsWith('.json')) continue;
    const file = path.join(detachedRoot, entry.name);
    const document = readJson(file);
    if (!document || typeof document !== 'object') continue;
    const stat = fs.statSync(file);
    const fallbackId = entry.name.slice(0, -'.json'.length);
    const id = typeof document.run_id === 'string' && document.run_id
      ? document.run_id
      : fallbackId;
    const action = typeof document.action === 'string' && document.action
      ? document.action
      : null;

    let status = 'recorded';
    if (document.status === 'running' || document.ok === null) status = 'running';
    else if (document.ok === true) status = 'succeeded';
    else if (document.ok === false || document.status === 'failed') status = 'failed';

    const resultCandidates = [
      document.result,
      document.result && typeof document.result === 'object' ? document.result.result : null,
    ];
    let result = null;
    for (const candidate of resultCandidates) {
      if (!candidate || typeof candidate !== 'object') continue;
      const healthy = typeof candidate.healthy === 'boolean' ? candidate.healthy : null;
      const signal = typeof candidate.signal === 'string' && candidate.signal ? candidate.signal : null;
      if (healthy !== null || signal) {
        result = {
          healthy,
          signal,
          recorded_at: typeof candidate.ts === 'string'
            ? candidate.ts
            : (typeof document.completed_at === 'string' ? document.completed_at : null),
        };
        break;
      }
    }

    const release = typeof document.release_version === 'string' && document.release_version
      ? document.release_version
      : null;
    const durable = {
      id,
      release,
      platform: process.platform,
      action,
      status,
      started_at: typeof document.started_at === 'string'
        ? document.started_at
        : isoOrNull(stat.birthtimeMs),
      completed_at: typeof document.completed_at === 'string' ? document.completed_at : null,
      updated_at: isoOrNull(stat.mtimeMs),
      artifact_counts: { screenshots: 0, pages: 0, videos: 0, logs: 0, records: 0, other: 0 },
      artifact_bytes: 0,
      cost_usd: null,
      result,
      uploaded: false,
      upload_proof: null,
    };
    const live = describedById.get(id);
    describedById.set(id, live
      ? {
          ...live,
          action: live.action ?? durable.action,
          status: durable.status === 'recorded' ? live.status : durable.status,
          started_at: live.started_at ?? durable.started_at,
          completed_at: durable.completed_at ?? live.completed_at,
          updated_at: durable.updated_at ?? live.updated_at,
          result: durable.result ?? live.result,
        }
      : durable);
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}

const allDescribed = [...describedById.values()].sort(
  (left, right) => (Date.parse(right.updated_at ?? '') || 0) - (Date.parse(left.updated_at ?? '') || 0),
);
const runTotal = allDescribed.length;
const described = allDescribed.slice(0, runLimit);

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

/// Inspect the images rendered by one HTTPS surface in a read-only Weles
/// browser session on TARGET.
pub async fn weles_image_inspect(
    target: &str,
    source_url: &str,
    json: bool,
) -> Result<(), CmdError> {
    let parsed = url::Url::parse(source_url)
        .map_err(|error| CmdError::usage(format!("--url is not a URL: {error}")))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(CmdError::usage(
            "--url must be an HTTPS URL without embedded credentials",
        ));
    }
    let source_url = parsed.to_string();
    let host = parsed.host_str().expect("checked above");
    let objective = "Inspect this public page without signing in or changing any user or application state. Scroll through the whole page to trigger lazy-loaded media. Inspect every rendered img element and report its currentSrc URL host and pathname, complete flag, naturalWidth and naturalHeight. Inspect PerformanceResourceTiming entries for image resources and /api/stado/object requests, including responseStatus where Chromium exposes it. Count loaded and failed images, count /api/stado/object image URLs, list every failed URL or HTTP status, and list any visible image-error placeholder text and the affected card or room name. Return one concise JSON object containing final_url, rendered_images, loaded_images, failed_images, stado_object_images, failed_resources, placeholders, and observations. Do not click controls that mutate data, create an account, or submit forms.";
    let admission = crate::deploy::weles_capture::resolve_admission(target)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let channel = crate::deploy::weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let result = crate::deploy::weles_capture::observe_action_payload(
        &channel,
        "generic_browser_task",
        json!({
            "url": source_url.as_str(),
            "objective": objective,
            "flow_name": format!("stado-image-inspection:{host}"),
            "session_label": format!("stado-image-inspection-{host}"),
            "proxy": "none",
            "headless": true,
            "constraints": {
                "read_only": true,
                "no_login": true,
                "no_mutation": true,
            },
        }),
        None,
        false,
    )
    .await
    .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let run_id = result
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click(format!("{target}: Weles returned no diagnostic run id")))?;
    let diagnostics = crate::deploy::weles_capture::image_diagnostics(&channel, run_id)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let task_result = result.get("result").cloned().unwrap_or(Value::Null);
    let browser_run = json!({
        "run_id": run_id,
        "trajectory_ok": result.get("ok").and_then(Value::as_bool).unwrap_or(false),
        "exit_code": result.get("exitCode"),
        "final_url": task_result.get("final_url"),
        "trajectory_error": task_result.get("error"),
    });
    let report = json!({
        "target": target,
        "source_url": source_url.as_str(),
        "action": "generic_browser_task",
        "endpoint": admission.declared_url,
        "transport": channel.transport(),
        "admission_token": channel.token_state(),
        "browser_run": browser_run,
        "images": diagnostics,
    });
    if json {
        print_json(&report);
    } else {
        println!(
            "{target}: inspected {} through {}",
            report["source_url"].as_str().unwrap_or(source_url.as_str()),
            admission.declared_url,
        );
        print_json(&diagnostics);
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
/// `stado host weles-browser-runtime` — verify, and optionally complete, the
/// browser runtime a Weles host needs.
///
/// Verify always runs first and runs again after a repair, so the command
/// reports the host's state rather than the installer's exit code: an install
/// that printed success and left the marker absent is the failure this whole
/// family of commands exists to catch.
pub async fn weles_browser_runtime(
    target: &str,
    components: &[String],
    repair: bool,
    json: bool,
) -> Result<(), CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let declared = crate::deploy::weles_browser_runtime::requirements(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    // One set decides both what is judged and what a repair installs, so the
    // verdict can never fail on something --repair would not have fixed.
    let required: Vec<String> = if components.is_empty() {
        vec![crate::deploy::weles_browser_runtime::DEFAULT_COMPONENT.to_string()]
    } else {
        components.to_vec()
    };
    let mut report =
        crate::deploy::weles_browser_runtime::verify(&resolved, &declared, &required, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;

    let mut installed: Vec<String> = Vec::new();
    if repair {
        installed = crate::deploy::weles_browser_runtime::repair(&resolved, &required, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        // Re-verify: the host's own answer decides, not the installer's.
        report =
            crate::deploy::weles_browser_runtime::verify(&resolved, &declared, &required, &runner)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
    }

    if json {
        let mut object = report.to_report(&resolved.name);
        object.insert("repaired".to_string(), serde_json::json!(installed));
        println!("{}", serde_json::to_string_pretty(&Value::Object(object))?);
    } else {
        println!("host:     {}", resolved.name);
        println!("runtime:  {}", report.verdict());
        for line in &installed {
            println!("repair:   {line}");
        }
        super::table::print(
            &["COMPONENT", "REVISION", "DEFAULT", "STATE", "EXPECTED AT"],
            &report
                .components
                .iter()
                .map(|component| {
                    vec![
                        component.name.clone(),
                        component.revision.clone(),
                        component.install_by_default.to_string(),
                        component.state.clone(),
                        component.expected_path.clone(),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }
    match report.failure(&resolved.name) {
        Some(reason) => Err(CmdError::click(reason)),
        None => Ok(()),
    }
}

/// `stado host mobile-runtime TARGET [--repair] [--json]` — verify, and
/// optionally install, the mobile automation runtime a host declares.
///
/// A host that declares no runtime exits zero after saying so. That is not a
/// pass rounded out of silence: `mobile_runtime` absent means the host is not
/// a mobile placement, and the alternative — failing every host in the fleet
/// against a runtime two of them need — is how an operator learns to write
/// `|| true` after the command, which is the argument
/// [`crate::host_software`] makes about programs nothing declares.
pub async fn mobile_runtime(target: &str, repair: bool, json: bool) -> Result<(), CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let Some(declared) = crate::deploy::mobile_runtime::requirement(&resolved).cloned() else {
        if json {
            print_json(&json!({
                "status": crate::deploy::mobile_runtime::OK_STATUS,
                "target": resolved.name,
                "runtime": "undeclared",
                "components": [],
            }));
        } else {
            println!(
                "{}: declares no mobile_runtime, so nothing is required of it here. Declare one \
                 in the registry target to place a mobile capture family on this host",
                resolved.name
            );
        }
        return Ok(());
    };
    let mut report = crate::deploy::mobile_runtime::verify(&resolved, &declared, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;

    let mut installed: Vec<String> = Vec::new();
    if repair {
        installed = crate::deploy::mobile_runtime::repair(&resolved, &declared, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        // Re-verify: the host's own answer decides, not the installer's.
        report = crate::deploy::mobile_runtime::verify(&resolved, &declared, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    }

    if json {
        let mut object = report.to_report(&resolved.name);
        object.insert("repaired".to_string(), json!(installed));
        println!("{}", serde_json::to_string_pretty(&Value::Object(object))?);
    } else {
        println!("host:     {}", resolved.name);
        println!("runtime:  {}", report.verdict());
        for line in &installed {
            println!("repair:   {line}");
        }
        super::table::print(
            &["COMPONENT", "DECLARED", "OBSERVED", "STATE", "RESOLVED AT"],
            &report
                .components
                .iter()
                .map(|component| {
                    vec![
                        component.name.clone(),
                        component.declared.clone(),
                        if component.observed.is_empty() {
                            "-".to_string()
                        } else {
                            component.observed.clone()
                        },
                        component.state.clone(),
                        component.path.clone(),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }
    match report.failure(&resolved.name) {
        Some(reason) => Err(CmdError::click(reason)),
        None => Ok(()),
    }
}

/// `stado host mobile-placement [--family ios|android] [--json]` — which
/// hosts a mobile capture family may be placed on.
///
/// Read out of the registry's declarations and nothing else, contacting no
/// host. That is the point of it: before this existed, the way to find out
/// whether a host could take the iOS family was to ask the host, and a
/// refusal from a machine that cannot run the family at all was
/// indistinguishable from a fleet-wide policy gap — which is exactly how the
/// four crawl families spent 2026-09-03 blocked on a question nobody could
/// answer. A host that declares no runtime for the family is not in the
/// answer, so it is never asked.
///
/// An empty answer exits non-zero and names the capability: no host declaring
/// the family is a state to act on, not a quiet zero.
pub async fn mobile_placement(family: Option<&str>, json: bool) -> Result<(), CmdError> {
    if let Some(asked) = family {
        if crate::deploy::mobile_runtime::family_driver(asked).is_none() {
            return Err(CmdError::usage(format!(
                "{asked:?} is not a mobile capture family; this build carries {}",
                crate::deploy::mobile_runtime::FAMILIES
                    .iter()
                    .map(|(name, driver)| format!("{name} (driver {driver})"))
                    .collect::<Vec<String>>()
                    .join(", ")
            )));
        }
    }
    let registry = load_registry_by_source("auto").await?;
    let placements = crate::deploy::mobile_runtime::placements(&registry, family);
    if json {
        print_json(&json!({
            "status": "mobile_placement",
            "capability": crate::deploy::mobile_runtime::CAPABILITY_ID,
            "family": family,
            "placements": placements,
        }));
    } else if placements.is_empty() {
        println!(
            "no registry host declares the {} capability for {}",
            crate::deploy::mobile_runtime::CAPABILITY_ID,
            family.unwrap_or("any mobile capture family"),
        );
    } else {
        super::table::print(
            &[
                "FAMILY",
                "HOST",
                "DRIVER",
                "APPIUM",
                "RESOLVE APPIUM AT",
                "RESOLVE ADB AT",
            ],
            &placements
                .iter()
                .map(|placement| {
                    vec![
                        placement.family.clone(),
                        placement.host.clone(),
                        placement.driver.clone(),
                        placement.appium.clone(),
                        placement.appium_paths.join(" "),
                        if placement.adb_paths.is_empty() {
                            "-".to_string()
                        } else {
                            placement.adb_paths.join(" ")
                        },
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }
    if placements.is_empty() {
        return Err(CmdError::click(format!(
            "no host declares {} for {}; declare targets[].mobile_runtime with the family's \
             driver on a host that can carry it",
            crate::deploy::mobile_runtime::CAPABILITY_ID,
            family.unwrap_or("any mobile capture family"),
        )));
    }
    Ok(())
}

/// What one `host capability-route` invocation asks for.
pub struct CapabilityRouteRequest<'a> {
    pub target: &'a str,
    pub resource: Option<&'a str>,
    pub item: Option<&'a str>,
    pub field: Option<&'a str>,
    pub reason: Option<&'a str>,
    pub verify: bool,
    /// Address a named broker instance instead of the host's default files.
    pub capability_file: Option<&'a str>,
    pub routes_file: Option<&'a str>,
    pub json: bool,
}

/// `stado host capability-route` — the fleet's own surface for the table that
/// decides which credential a login form receives, on the host that holds it.
///
/// Read without `--resource`, declare with all four flags: the same shape
/// `retag-vault-item` uses, so an operator about to change a route can first
/// see what they would be changing. Nothing secret crosses either way: a
/// route names coordinates, never a value.
pub async fn capability_route(request: CapabilityRouteRequest<'_>) -> Result<(), CmdError> {
    let CapabilityRouteRequest {
        target,
        resource,
        item,
        field,
        reason,
        verify,
        json,
        capability_file,
        routes_file,
    } = request;
    // Declaring takes all four or none of them. A partial declaration is the
    // one input that could look like a read and write something.
    let declaration =
        match (resource, item, field, reason) {
            (None, None, None, None) => None,
            (Some(resource), Some(item), Some(field), Some(reason)) => {
                if reason.trim().is_empty() {
                    return Err(CmdError::usage(
                    "--reason must say why this route exists; it travels into Skarbiec's journal \
                     beside the table",
                ));
                }
                Some((resource, item, field, reason))
            }
            (None, _, _, _) => {
                return Err(CmdError::usage(
                    "reading takes only TARGET; declaring takes --resource, --item, --field and \
                 --reason together",
                ))
            }
            _ => return Err(CmdError::usage(
                "declaring one capability route takes --resource, --item, --field and --reason \
                 together",
            )),
        };

    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let broker = crate::deploy::host_capability::resolve(
        &resolved,
        &crate::deploy::host_capability::BrokerFiles {
            capability_file,
            routes_file,
        },
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;

    if verify && declaration.is_some() {
        return Err(CmdError::usage(
            "--verify is a read; it does not combine with declaring a route",
        ));
    }
    let report = match declaration {
        Some((resource, item, field, reason)) => crate::deploy::host_capability::route_add(
            &resolved, &broker, resource, item, field, reason, &runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?,
        None if verify => {
            crate::deploy::host_capability::verify_routes(&resolved, &broker, &runner)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?
        }
        None => crate::deploy::host_capability::routes(&resolved, &broker, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?,
    };

    if json {
        let mut object = crate::deploy::host_channel::base_report(&resolved);
        object.insert("vault".to_string(), json!(broker.vault));
        object.insert("report".to_string(), report);
        println!("{}", serde_json::to_string_pretty(&Value::Object(object))?);
        return Ok(());
    }
    println!("host:      {}", resolved.name);
    println!("vault:     {}", broker.vault);
    match report.get("added").and_then(Value::as_bool) {
        Some(true) => {
            println!(
                "declared:  {} -> {}/{}",
                report["resource"].as_str().unwrap_or_default(),
                report["item"].as_str().unwrap_or_default(),
                report["field"].as_str().unwrap_or_default(),
            );
            if let Some(backup) = report.get("backup").and_then(Value::as_str) {
                println!("backup:    {backup}");
            }
        }
        Some(false) => println!(
            "unchanged: {} already maps {}/{}",
            report["resource"].as_str().unwrap_or_default(),
            report["item"].as_str().unwrap_or_default(),
            report["field"].as_str().unwrap_or_default(),
        ),
        None if verify => {
            // `checked` plus one line per route that cannot deliver, in the
            // host's own words. An empty `broken` list is the whole point.
            println!(
                "checked:   {}",
                report.get("checked").and_then(Value::as_u64).unwrap_or(0)
            );
            let broken = report
                .get("broken")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            println!("broken:    {}", broken.len());
            for row in broken {
                println!(
                    "  {:<52} {}",
                    row["resource"].as_str().unwrap_or_default(),
                    row["problem"].as_str().unwrap_or_default(),
                );
            }
        }
        None => {
            let rows = report
                .get("routes")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            println!("routes:    {}", rows.len());
            for row in rows {
                println!(
                    "  {:<52} {}/{} item={} field={}",
                    row["resource"].as_str().unwrap_or_default(),
                    row["item"].as_str().unwrap_or_default(),
                    row["field"].as_str().unwrap_or_default(),
                    row["item_present"].as_bool().unwrap_or(false),
                    row["field_present"].as_bool().unwrap_or(false),
                );
            }
        }
    }
    Ok(())
}

/// What one `host weles-browser-task` invocation asks for.
pub struct BrowserTaskRequest<'a> {
    pub target: &'a str,
    pub url: &'a str,
    pub objective: &'a str,
    pub session_label: &'a str,
    pub action: &'a str,
    pub allowlist_file: &'a str,
    pub login_item: Option<&'a str>,
    /// The account identity that keys the browser profile, when the caller
    /// pins one so a later run can reuse its session.
    pub account_id: Option<&'a str>,
    pub fresh_profile: bool,
    pub allow_login: bool,
    pub sign_in_origin: Option<&'a str>,
    pub sign_in_item: Option<&'a str>,
    /// Give the agent every capability rather than prefilling the first: see
    /// the flag's own help for the runtime version this exists for.
    pub defer_fills: bool,
    /// Prefill both same-page sign-in fields before the agent runs.
    pub prefill_all: bool,
    /// The saved-trajectory key, when it must differ from the profile's label.
    pub flow_name: Option<&'a str>,
    pub windowed: bool,
    pub json: bool,
}

/// `stado host weles-browser-task` — the general Weles submission surface.
///
/// The allowlist check happens first and on its own round trip, because the
/// point is to refuse before anything is enqueued: a name the worker will not
/// run must produce a sentence here, not an accepted job that disappears.
pub async fn weles_browser_task(request: BrowserTaskRequest<'_>) -> Result<(), CmdError> {
    let BrowserTaskRequest {
        target,
        url,
        objective,
        session_label,
        action,
        allowlist_file,
        login_item,
        account_id,
        fresh_profile,
        allow_login,
        sign_in_origin,
        sign_in_item,
        defer_fills,
        prefill_all,
        flow_name,
        windowed,
        json,
    } = request;
    // Both halves or neither, and only where the run says it may sign in.
    // Checked before the host is resolved: a flag combination that cannot work
    // should cost nothing.
    let sign_in =
        match (sign_in_origin, sign_in_item) {
            (None, None) => None,
            (Some(_), None) => {
                return Err(CmdError::usage(
                    "--sign-in-origin needs --sign-in-item: the vault item holding the account",
                ))
            }
            (None, Some(_)) => return Err(CmdError::usage(
                "--sign-in-item needs --sign-in-origin: the page origin whose fields are filled",
            )),
            (Some(origin), Some(item)) => {
                if !allow_login {
                    return Err(CmdError::usage(
                    "--sign-in-origin requires --allow-login: a prefilled credential is a sign-in, \
                     and the run's own instructions would otherwise tell the agent not to",
                ));
                }
                let origin = crate::deploy::weles_browser_task::exact_origin(origin)
                    .map_err(|error| CmdError::usage(error.to_string()))?;
                Some((origin, item))
            }
        };
    let parsed = url::Url::parse(url)
        .map_err(|error| CmdError::usage(format!("--url is not a URL: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.username() != "" {
        return Err(CmdError::usage(
            "--url must be an HTTP or HTTPS URL without embedded credentials",
        ));
    }
    // `@path` keeps a long objective out of a shell history and out of argv.
    let objective = match objective.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|error| {
                CmdError::usage(format!("cannot read objective from {path}: {error}"))
            })?
            .trim()
            .to_string(),
        // Trimmed on both paths: an all-whitespace objective is not a task,
        // and only trimming the `@file` path made that depend on how the
        // objective was supplied.
        None => objective.trim().to_string(),
    };
    if objective.is_empty() {
        return Err(CmdError::usage("--objective is empty"));
    }
    if let Some(item) = login_item {
        let bytes = item.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 128
            || !bytes[0].is_ascii_alphanumeric()
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(CmdError::usage("--login-item is not a valid Weles item id"));
        }
        if !action.ends_with("_login") {
            return Err(CmdError::usage(
                "--login-item requires an action whose name ends in _login",
            ));
        }
        if !allow_login {
            return Err(CmdError::usage("--login-item requires --allow-login"));
        }
    }
    // A pinned identity is the caller's; otherwise a fresh profile still needs
    // one, because the API refuses `fresh_profile` without an account to bind
    // the new directory to.
    let account_id = match account_id {
        Some(pinned) => Some(
            crate::deploy::weles_capture::checked_account_id(pinned)
                .map_err(|error| CmdError::click(error.to_string()))?
                .to_string(),
        ),
        None => fresh_profile.then(|| format!("stado-fresh-profile-{}", uuid::Uuid::new_v4())),
    };

    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let allowlist =
        crate::deploy::weles_browser_task::host_allowlist(&resolved, allowlist_file, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    crate::deploy::weles_browser_task::ensure_allowed(&resolved.name, action, &allowlist)
        .map_err(|error| CmdError::click(error.to_string()))?;

    // Only now: a capability is single-use and expires, so it is issued after
    // the action has been shown to be one this host accepts, never before.
    // Issued ON that host, because redemption is a socket there.
    let (credential_prefill, credential_deferred) = match &sign_in {
        None => (Vec::new(), Vec::new()),
        Some((origin, item)) => {
            // The identity comes from the catalog this host's vault was
            // registered from, never from a constant here: Skarbiec looks a
            // capability's agent up by name, and a name it does not register
            // is denied however correct the route and the reference are.
            let prefill = crate::deploy::weles_browser_task::issue_sign_in_prefill(
                &resolved,
                origin,
                item,
                crate::deploy::weles_browser_task::REGISTERED_SCOPES_FILE,
                &runner,
            )
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
            if !json {
                println!("sign-in:   {origin} as the account in {item}");
                println!(
                    "prefill:   {} field(s) to {}, issued on {}, single-use",
                    prefill.entries.len(),
                    prefill.agents.join(", "),
                    resolved.name
                );
                if !prefill.deferred.is_empty() {
                    println!(
                        "deferred:  {} field(s) handed over unspent, for the page that has them",
                        prefill.deferred.len()
                    );
                }
                if !prefill.unconfirmed.is_empty() {
                    println!(
                        "note:      this channel could not open {} to confirm the field; the \
                         worker's own broker reads it at fill time (`host capability-route {} \
                         --verify` says why)",
                        prefill.unconfirmed.join(", "),
                        resolved.name
                    );
                }
            }
            if defer_fills {
                // Nothing is prefilled: on a runtime that fills at load without
                // waiting, a capability is spent whether or not the input has
                // rendered, and a spent one cannot be retried. The agent acts
                // after the page is up, so it can see the field it fills.
                let mut all = prefill.entries;
                all.extend(prefill.deferred);
                (Vec::new(), all)
            } else if prefill_all {
                // Same-page forms can be completed without a model handling
                // either credential. Weles 0.5.41+ checks field presence before
                // redemption, so an absent later field remains available in
                // the constraints for the agent.
                let mut all = prefill.entries;
                all.extend(prefill.deferred);
                (all, Vec::new())
            } else {
                (prefill.entries, prefill.deferred)
            }
        }
    };

    let task = crate::deploy::weles_browser_task::BrowserTask {
        action,
        url: parsed.as_str(),
        objective: &objective,
        session_label,
        login_item,
        account_id: account_id.as_deref(),
        fresh_profile,
        allow_login,
        headless: !windowed,
        credential_prefill,
    };
    let outcome =
        crate::deploy::weles_browser_task::submit(target, &task, flow_name, &credential_deferred)
            .await
            .map_err(|error| CmdError::click(format!("{target}: {error}")))?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Object(
                outcome.to_report(&resolved.name, action)
            ))?
        );
    } else {
        println!("host:      {}", resolved.name);
        println!("action:    {action}");
        println!("run:       {}", outcome.run_id);
        println!("outcome:   {}", if outcome.ok { "ok" } else { "failed" });
        if let Some(code) = outcome.exit_code {
            println!("exit:      {code}");
        }
        if let Some(profile) = &outcome.profile {
            println!(
                "profile:   {}",
                profile["directory"].as_str().unwrap_or("fresh")
            );
        }
        if !outcome.result.is_null() {
            println!("result:    {}", serde_json::to_string(&outcome.result)?);
        }
    }
    if outcome.ok {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "{}: {action} run {} did not succeed",
            resolved.name, outcome.run_id
        )))
    }
}

/// Read one completed browser run through Weles' authenticated diagnostic API.
pub async fn weles_run_diagnostics(
    target: &str,
    run_id: &str,
    file: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let admission = crate::deploy::weles_capture::resolve_admission(target)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let channel = crate::deploy::weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let Some(path) = file else {
        let manifest = crate::deploy::weles_capture::run_diagnostics(&channel, run_id)
            .await
            .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
        print_json(&manifest);
        return Ok(());
    };
    let bytes = crate::deploy::weles_capture::run_diagnostic_file(&channel, run_id, path)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let byte_count = bytes.len();
    let (encoding, content) = match String::from_utf8(bytes) {
        Ok(text) => ("utf8", text),
        Err(error) => ("base64", STANDARD.encode(error.into_bytes())),
    };
    if json_output {
        print_json(&json!({
            "target": target,
            "run_id": run_id,
            "path": path,
            "bytes": byte_count,
            "encoding": encoding,
            "content": content,
        }));
    } else if encoding == "utf8" {
        print!("{content}");
    } else {
        println!("base64:{content}");
    }
    Ok(())
}

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
/// The `-R` specification `forward_local` gave ssh, which is what identifies
/// one channel among several on this machine.
///
/// Matching on this and the destination, never on the program name: this host
/// runs other forwards, and `pkill ssh` would take the fleet's other channels
/// down with the one being closed.
fn reverse_forward_spec(remote_port: u16, local_port: u16) -> String {
    format!("127.0.0.1:{remote_port}:127.0.0.1:{local_port}")
}
fn local_forward_spec_prefix(local_port: u16) -> String {
    format!("127.0.0.1:{local_port}:127.0.0.1:")
}

/// `stado host forward-close TARGET NAME` — end a channel opened by either
/// `forward-local` or `forward-remote` and reconcile its markers.
///
/// Order matters. The process is ended first, then the markers are removed,
/// then the exposed port is re-read: a marker deleted while the tunnel still
/// carried traffic would leave a live port nothing describes, which is worse
/// than the stale marker it was trying to fix.
pub async fn forward_close(target: &str, name: &str, json: bool) -> Result<(), CmdError> {
    release_component("forward name", name)?;
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    let local_marker = std::path::Path::new(&home)
        .join(".stado")
        .join("forwards")
        .join(format!("{name}.local"));
    let local_forward_marker = std::path::Path::new(&home)
        .join(".stado")
        .join("forwards")
        .join(format!("{name}.url"));

    // The ports come from the markers the open wrote, so a close addresses the
    // exact channel that was opened rather than a port an operator remembers.
    let local_port = std::fs::read_to_string(&local_marker)
        .ok()
        .and_then(|body| url::Url::parse(body.trim()).ok())
        .and_then(|parsed| parsed.port());
    let remote_marker_body = crate::deploy::service_file_fetch::fetch_file(
        &resolved,
        &format!("$HOME/.stado/forwards/{name}.url"),
        &runner,
    )
    .await
    .ok()
    .filter(|fetched| fetched.ok())
    .map(|fetched| String::from_utf8_lossy(&fetched.content).trim().to_string());
    let remote_port = remote_marker_body
        .as_deref()
        .and_then(|body| url::Url::parse(body).ok())
        .and_then(|parsed| parsed.port());
    // `forward-remote` has no marker on TARGET. Its local `.url` marker is
    // considered only when neither half of a reverse forward exists, so the
    // same name can never make this close the wrong direction.
    let local_forward_port = if local_port.is_none() && remote_port.is_none() {
        std::fs::read_to_string(&local_forward_marker)
            .ok()
            .and_then(|body| url::Url::parse(body.trim()).ok())
            .and_then(|parsed| parsed.port())
    } else {
        None
    };

    if local_port.is_none() && remote_port.is_none() && local_forward_port.is_none() {
        return Err(CmdError::click(format!(
            "{target}: no forward named {name:?} is recorded here or on the host, so there is \
             nothing to close; `stado host inventory {target}` lists the markers that exist"
        )));
    }

    // End the ssh that carries it, matched on the whole -R spec.
    let mut ended: Vec<String> = Vec::new();
    if let (Some(remote), Some(local)) = (remote_port, local_port) {
        let spec = reverse_forward_spec(remote, local);
        let listing = tokio::process::Command::new("/bin/ps")
            .args(["ax", "-o", "pid=", "-o", "command="])
            .output()
            .await?;
        for line in String::from_utf8_lossy(&listing.stdout).lines() {
            let trimmed = line.trim_start();
            let Some((pid, command)) = trimmed.split_once(char::is_whitespace) else {
                continue;
            };
            if !command.contains(&spec) || !command.contains("ssh") {
                continue;
            }
            let Ok(parsed) = pid.parse::<i32>() else {
                continue;
            };
            let killed = tokio::process::Command::new("/bin/kill")
                .args(["-TERM", &parsed.to_string()])
                .output()
                .await?;
            ended.push(format!(
                "pid {parsed}{}",
                if killed.status.success() {
                    ""
                } else {
                    " (signal refused)"
                }
            ));
        }
    }
    // A local forward is identified by the complete local half of its `-L`
    // specification and the registry target's exact SSH destination. Matching
    // either one alone could end an unrelated channel.
    if let Some(local) = local_forward_port {
        let destinations = resolved
            .ssh_connections()
            .map(|(_, destination)| destination.to_string())
            .collect::<Vec<_>>();
        if destinations.is_empty() {
            return Err(CmdError::click(
                "registry target has no SSH connection path",
            ));
        }
        let spec_prefix = local_forward_spec_prefix(local);
        let listing = tokio::process::Command::new("/bin/ps")
            .args(["ax", "-o", "pid=", "-o", "command="])
            .output()
            .await?;
        for line in String::from_utf8_lossy(&listing.stdout).lines() {
            let trimmed = line.trim_start();
            let Some((pid, command)) = trimmed.split_once(char::is_whitespace) else {
                continue;
            };
            if !command.contains("ssh")
                || !command.contains(" -L ")
                || !command.contains(&spec_prefix)
                || !destinations
                    .iter()
                    .any(|destination| command.contains(destination))
            {
                continue;
            }
            let Ok(parsed) = pid.parse::<i32>() else {
                continue;
            };
            let killed = tokio::process::Command::new("/bin/kill")
                .args(["-TERM", &parsed.to_string()])
                .output()
                .await?;
            ended.push(format!(
                "pid {parsed}{}",
                if killed.status.success() {
                    ""
                } else {
                    " (signal refused)"
                }
            ));
        }
    }

    // Then the markers, remote first for a reverse forward: it is the one
    // another machine reads.
    let remote_removed = if remote_marker_body.is_some() {
        let script = format!(
            "set -eu\n/bin/rm -f \"$HOME/.stado/forwards/\"{name}\".url\"\n",
            name = crate::deploy::shlex_quote(name)
        );
        let removed = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        if !removed.ok() {
            return Err(CmdError::click(format!(
                "{target}: the channel was ended and its host marker could not be removed, so \
                 the host still advertises an endpoint: {}",
                crate::deploy::host_channel::last_error_line(
                    &removed,
                    "remote marker removal failed"
                )
            )));
        }
        true
    } else {
        false
    };
    let local_removed = if local_forward_port.is_some() {
        std::fs::remove_file(&local_forward_marker).is_ok()
    } else {
        std::fs::remove_file(&local_marker).is_ok()
    };

    // Re-read the side that was exposed. The endpoint itself, not a process
    // lookup, decides whether the port was reclaimed.
    let still_listening = if let Some(port) = local_forward_port {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        Some(
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                tokio::net::TcpStream::connect(("127.0.0.1", port)),
            )
            .await
            .map_or(true, |result| result.is_ok()),
        )
    } else {
        match remote_port {
            Some(port) => {
                let report = crate::deploy::service_serving::read_serving(
                    &resolved,
                    "com.wisent.host-health-beacon",
                    "",
                    &[port],
                    &runner,
                )
                .await
                .ok();
                report.map(|report| {
                    report
                        .ports
                        .first()
                        .is_some_and(|entry| !entry.holders.is_empty())
                })
            }
            None => None,
        }
    };
    let effective_local_port = local_forward_port.or(local_port);
    let direction = if local_forward_port.is_some() {
        "local"
    } else {
        "reverse"
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "name": name,
                "direction": direction,
                "remote_port": remote_port,
                "local_port": effective_local_port,
                "ended": ended,
                "remote_marker_removed": remote_removed,
                "local_marker_removed": local_removed,
                "port_still_listening": still_listening,
                "remote_port_still_listening": if local_forward_port.is_none() { still_listening } else { None },
                "local_port_still_listening": if local_forward_port.is_some() { still_listening } else { None },
            }))?
        );
    } else {
        println!("host:      {}", resolved.name);
        println!("forward:   {name}");
        println!("direction: {direction}");
        println!(
            "ports:     remote {} local {}",
            remote_port.map_or_else(|| "-".to_string(), |port| port.to_string()),
            effective_local_port.map_or_else(|| "-".to_string(), |port| port.to_string())
        );
        println!(
            "ended:    {}",
            if ended.is_empty() {
                "no matching ssh process on this machine".to_string()
            } else {
                ended.join(", ")
            }
        );
        println!(
            "markers:  host {} local {}",
            if remote_removed { "removed" } else { "absent" },
            if local_removed { "removed" } else { "absent" }
        );
        println!(
            "port:     {}",
            match still_listening {
                Some(true) if local_forward_port.is_some() => "STILL LISTENING locally",
                Some(true) => "STILL LISTENING on the host",
                Some(false) => "reclaimed",
                None => "unverified",
            }
        );
    }
    match still_listening {
        Some(true) => Err(CmdError::click(format!(
            "{}: {name} was closed and its {} port is still listening, so something else holds \
             it; the markers are gone and the port is not this forward's any more",
            resolved.name,
            if local_forward_port.is_some() {
                "local"
            } else {
                "remote"
            }
        ))),
        _ => Ok(()),
    }
}

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
    let runner = crate::deploy::production_runner();
    let connection = crate::deploy::host_channel::select_ssh_connection(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let connection_path = connection.name.to_string();
    let mut argv = crate::deploy::host_channel::ssh_options(connection.destination);
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
    let key = crate::deploy::ssh_key::materialize(&resolved.name)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let argv = crate::deploy::ssh_key::add_identity(argv, &key)
        .map_err(|error| CmdError::click(error.to_string()))?;
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
                "connection_path": connection_path,
                "status": "forwarding",
            }))?
        );
    } else {
        println!(
            "{target}: forwarding 127.0.0.1:{remote_port} to local 127.0.0.1:{local_port} over SSH via {connection_path}"
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
    let runner = crate::deploy::production_runner();
    let connection = crate::deploy::host_channel::select_ssh_connection(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let connection_path = connection.name.to_string();
    let mut argv = crate::deploy::host_channel::ssh_options(connection.destination);
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
        "-L".to_string(),
        format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}"),
        destination,
    ]);
    let key = crate::deploy::ssh_key::materialize(&resolved.name)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let argv = crate::deploy::ssh_key::add_identity(argv, &key)
        .map_err(|error| CmdError::click(error.to_string()))?;
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
            "{target}: SSH forwarding failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next_back()
                .unwrap_or("ssh forwarding failed")
        )));
    }
    let endpoint = format!("http://127.0.0.1:{local_port}");
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    let marker_directory = std::path::Path::new(&home).join(".stado").join("forwards");
    std::fs::create_dir_all(&marker_directory)?;
    let marker_path = marker_directory.join(format!("{name}.url"));
    std::fs::write(&marker_path, format!("{endpoint}\n"))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "remote": format!("127.0.0.1:{remote_port}"),
                "local": format!("127.0.0.1:{local_port}"),
                "marker": marker_path,
                "transport": "ssh",
                "connection_path": connection_path,
                "status": "forwarding",
            }))?
        );
    } else {
        println!(
            "{target}: forwarding local 127.0.0.1:{local_port} to 127.0.0.1:{remote_port} over SSH via {connection_path}"
        );
    }
    Ok(())
}

/// The replica roots every fleet host uses, relative to the managed home. Both
/// are the values the service catalog declares for the object API unit
/// (`WC_LOCAL_STORAGE_PATH`) and its replica, so this command reads the same
/// layout the fleet installs rather than asking an operator to name a path.
const BACKUP_ROOT: &str = ".stado/local-backup";
const PRIMARY_ROOT: &str = ".stado/local-storage";

/// Classify a host's local replica against the store it mirrors, and reclaim
/// the twins when asked.
///
/// The first time a tree on this fleet was assumed to be duplicate data it
/// turned out to be the only copy of 9.58 GiB, so classifying is the default
/// and it deletes nothing. A reclaim proves and deletes inside ONE pass: every
/// object it unlinks was hashed on both sides moments earlier by that same
/// pass. It never reads a verdict from a previous run, which is the shape that
/// turns a replica into data loss when addresses move between the audit and
/// the deletion — and they did move on this host, twice, in one evening.
pub async fn backup_audit(
    target: &str,
    reclaim_twins: bool,
    apply: bool,
    json: bool,
) -> Result<(), CmdError> {
    let namespace = crate::config::wc_stado_storage_namespace();
    if namespace.trim().is_empty() {
        return Err(CmdError::click(
            "this control plane has no storage.stado.namespace, so a replica path cannot be \
             resolved to a primary address",
        ));
    }
    let plan = crate::deploy::host_backup_audit::AuditPlan {
        namespace: namespace.to_string(),
        backup_root: BACKUP_ROOT.to_string(),
        primary_root: PRIMARY_ROOT.to_string(),
        reclaim: reclaim_twins,
        apply,
    };
    let runner = crate::deploy::production_runner();
    let (_, audit) = crate::deploy::host_backup_audit::audit_host(target, &plan, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let gib = |bytes: u64| bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    if json {
        let classes: serde_json::Map<String, Value> = audit
            .classes
            .iter()
            .map(|(class, totals)| {
                (
                    class.clone(),
                    json!({"objects": totals.objects, "bytes": totals.bytes}),
                )
            })
            .collect();
        print_json(&json!({
            "host": audit.host,
            "complete": audit.complete,
            "unavailable": audit.unavailable,
            "classes": classes,
            "reclaimable_bytes": audit.reclaimable_bytes(),
            "retained_bytes": audit.retained_bytes(),
            "reclaim": {
                "requested": reclaim_twins,
                "applied": reclaim_twins && apply,
                "complete": audit.reclaim_complete,
                "deleted_objects": audit.deleted.objects,
                "deleted_bytes": audit.deleted.bytes,
                "would_delete_objects": audit.would_delete.objects,
                "would_delete_bytes": audit.would_delete.bytes,
                "delete_failed_objects": audit.delete_failed.objects,
                "delete_failed_bytes": audit.delete_failed.bytes,
                "pruned_directories": audit.pruned_directories,
            },
            "free_kb_before": audit.free_kb_before,
            "free_kb_after": audit.free_kb_after,
        }));
        return Ok(());
    }
    for class in [
        crate::deploy::host_backup_audit::TWIN,
        crate::deploy::host_backup_audit::ABSENT,
        crate::deploy::host_backup_audit::DIFFERS,
        crate::deploy::host_backup_audit::SAME_SIZE_UNPROVEN,
    ] {
        let totals = audit.classes.get(class).cloned().unwrap_or_default();
        println!(
            "{class:9} {:>7} object(s)  {:>8.2} GiB",
            totals.objects,
            gib(totals.bytes)
        );
        for (bytes, path) in audit.examples.get(class).into_iter().flatten() {
            println!("          {:>8.2} GiB  {path}", gib(*bytes));
        }
    }
    println!(
        "reclaim:  {:.2} GiB proven present and identical in the primary; {:.2} GiB is data and stays",
        gib(audit.reclaimable_bytes()),
        gib(audit.retained_bytes())
    );
    // The free-space pair the pass read itself, on both sides of its own
    // deletions. Reported even for a read-only classification, because "how
    // full is this disk while you are telling me what is on it" is the
    // question the whole command exists to serve.
    let gib_kb = |blocks: i64| blocks as f64 / 1024.0 / 1024.0;
    if let (Some(before), Some(after)) = (audit.free_kb_before, audit.free_kb_after) {
        println!(
            "free:     {:.2} GiB before, {:.2} GiB after ({:+.2} GiB)",
            gib_kb(before),
            gib_kb(after),
            gib_kb(after - before),
        );
    }
    if reclaim_twins {
        if apply {
            println!(
                "deleted:  {} object(s)  {:.2} GiB, each one hashed on both sides by this pass; \
                 {} emptied directories removed",
                audit.deleted.objects,
                gib(audit.deleted.bytes),
                audit.pruned_directories,
            );
            if audit.delete_failed.objects > 0 {
                println!(
                    "refused:  {} object(s) the host would not unlink; they are still in the replica",
                    audit.delete_failed.objects
                );
            }
            if !audit.reclaim_complete {
                println!(
                    "warning:  the reclaim half did not print its own end marker, so the deleted \
                     count is a floor; run the command again"
                );
            }
        } else {
            println!(
                "would delete: {} object(s)  {:.2} GiB — nothing was changed; pass --apply, which \
                 re-proves every one of them in that same pass",
                audit.would_delete.objects,
                gib(audit.would_delete.bytes),
            );
        }
    }
    if !audit.complete {
        println!(
            "warning:  the host did not finish classifying, so these totals are a floor, not the answer"
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
        let connection = crate::deploy::host_channel::select_ssh_connection(&resolved, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let ssh_target = connection.destination;
        let mut options = crate::deploy::host_channel::ssh_options(ssh_target);
        options.pop();
        let mut argv = vec!["scp".to_string(), "-q".to_string()];
        argv.extend(options.into_iter().skip(usize::from(true)));
        argv.push(source.to_string());
        argv.push(format!("{ssh_target}:{staged}"));
        let key = crate::deploy::ssh_key::materialize(&resolved.name)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let argv = crate::deploy::ssh_key::add_identity(argv, &key)
            .map_err(|error| CmdError::click(error.to_string()))?;
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

/// Read the effective configuration on a fleet host using the same installed
/// Stado binary and config path its services consume.
pub async fn config_show(target: &str) -> Result<(), CmdError> {
    remote_config(target, None).await
}

/// Persist one configuration field on a fleet host. Values travel base64
/// encoded inside the audited script and are decoded into argv, never parsed by
/// a remote shell. When `reload_service` is named, the existing service
/// reconciler activates the new configuration only after the atomic write
/// succeeds.
pub async fn config_set(
    target: &str,
    key: &str,
    value: &str,
    reload_service: Option<&str>,
) -> Result<(), CmdError> {
    if key.trim().is_empty() || key.chars().any(char::is_whitespace) {
        return Err(CmdError::click(
            "configuration key must be a non-empty dotted name",
        ));
    }
    remote_config(target, Some((key, value))).await?;
    warn_unbacked_object_namespace(target, key, value);
    if let Some(service) = reload_service {
        super::service::reconcile_after_config_change(
            service,
            target,
            &format!("managed configuration {key} changed"),
        )
        .await?;
    }
    Ok(())
}

/// Say, at declaration time, what an object namespace without a grant costs.
///
/// `object_api.namespaces.<ns>` names a Skarbiec item, and the host's object
/// verifier must hold a read on it or the whole object authorization boundary
/// closes — not just that namespace. On 2026-09-03 `spis-crawls` was declared
/// on `charless-mac-mini` with its item `spis-crawls-object-api` outside the
/// verifier's grant. Nothing complained. The boundary closed, every non-release
/// object read answered `503 object authorization unavailable`, and the fault
/// stayed invisible until the next restart of the release agent — which then
/// could not read `release_control`, published no stable bind, and left
/// `brama.wisent.com/health` answering 502 for hours. The log line that named
/// it, `object verifier grant item set mismatch (missing=[spis-crawls-object-api])`,
/// existed the whole time on the host and nowhere an operator was looking.
///
/// So the warning is emitted here, where the declaration is made, and it names
/// the second half of the trap too: `reconcile-object-verifier` computes the
/// item set from the configuration of the machine running it, so a namespace
/// that exists only on the host can never be satisfied from here. That is why
/// the sentence asks for the declaration on both sides.
fn warn_unbacked_object_namespace(target: &str, key: &str, value: &str) {
    let Some(namespace) = key.strip_prefix("object_api.namespaces.") else {
        return;
    };
    if namespace.is_empty() || namespace.contains('.') {
        return;
    }
    let item = serde_json::from_str::<Value>(value)
        .ok()
        .and_then(|declared| {
            declared
                .get("item")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let Some(item) = item else {
        return;
    };
    let covered = crate::config::object_api_namespaces()
        .map(|namespaces| {
            crate::config::object_verifier_items(namespaces)
                .iter()
                .any(|held| held == &item)
        })
        .unwrap_or(false);
    if covered {
        eprintln!(
            "note: {target}'s object verifier grant must cover {item:?} for namespace \
             {namespace:?}; this machine declares it too, so reconcile the host with: stado host \
             reconcile-object-verifier {target}"
        );
        return;
    }
    eprintln!(
        "warning: namespace {namespace:?} on {target} names Skarbiec item {item:?}, and this \
         machine's own object_api.namespaces does not declare it. Until the host's object verifier \
         grant covers that item its WHOLE object authorization boundary closes — every \
         /api/object read answers 503, not just this namespace — and the failure surfaces at the \
         next restart of anything that reads the registry, including the release agent that \
         publishes every stable bind. `stado host reconcile-object-verifier {target}` computes the \
         item set from THIS machine's configuration, so declare the namespace here as well and \
         then run it: stado config set {key} '<the same JSON>' && stado host \
         reconcile-object-verifier {target}"
    );
}

async fn remote_config(target: &str, update: Option<(&str, &str)>) -> Result<(), CmdError> {
    let target = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let stdout = remote_config_output(&target, update, &crate::deploy::production_runner()).await?;
    print!("{stdout}");
    Ok(())
}

/// One `stado config show` on a fleet host — the effective configuration its
/// own installed binary resolves, from its own `STADO_CONFIG` — returned
/// instead of printed.
///
/// Factored out of [`remote_config`] so [`crate::deploy::host_gates`] can ask
/// a host which storage backend its queue agent is bound to without a second
/// remote script existing for the same question. Two scripts reading one
/// host's configuration would eventually read it two different ways — a
/// different `STADO_CONFIG`, a different binary, a different `HOME` — and the
/// answer that matters here is exactly "what does the config the services
/// consume say", which is what this one already asks.
///
/// The incident: the Mac mini's agent unit was re-declared with a
/// `STADO_CONFIG` naming a config that set `wc_storage_backend: "local"`, so
/// the agent published its capacity into an on-disk store on that machine and
/// nothing in the fleet ever read it. `host config-show` could see that field
/// the whole time; nothing that judged the host asked it.
pub(crate) async fn remote_config_output(
    target: &ComputeTarget,
    update: Option<(&str, &str)>,
    runner: &crate::deploy::Runner,
) -> Result<String, CmdError> {
    let action = match update {
        None => "\"$binary\" config show".to_string(),
        Some((key, value)) => format!(
            "key=\"$(printf '%s' '{}' | /usr/bin/base64 \"$decode\")\"\n\
             value=\"$(printf '%s' '{}' | /usr/bin/base64 \"$decode\")\"\n\
             \"$binary\" config set \"$key\" \"$value\"\n\
             \"$binary\" config show",
            STANDARD.encode(key.as_bytes()),
            STANDARD.encode(value.as_bytes())
        ),
    };
    let script = format!(
        "set -euo pipefail\n\
         case \"$(/usr/bin/uname -s)\" in Darwin) decode=-D ;; *) decode=--decode ;; esac\n\
         export STADO_CONFIG=\"$HOME/.config/stado/config.json\"\n\
         binary=\"$HOME/.stado/bin/stado\"\n\
         test -x \"$binary\"\n\
         {action}\n"
    );
    let output = crate::deploy::host_channel::run_script_with_timeout(
        target,
        &script,
        std::time::Duration::from_secs(60),
        runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(
            crate::deploy::host_channel::last_error_line(
                &output,
                "remote Stado configuration command failed",
            ),
        ));
    }
    Ok(output.stdout)
}

pub async fn verify_release_platform(
    target: &str,
    repo: &str,
    revision: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    if !repo.starts_with("https://") {
        return Err(CmdError::click("--repo must be an https:// clone URL"));
    }
    if revision.len() != 40
        || !revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(CmdError::click("--ref must be a full lowercase Git commit"));
    }
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let repo = crate::deploy::shlex_quote(repo);
    let revision = crate::deploy::shlex_quote(revision);
    let script = format!(
        r#"set -euo pipefail
export PATH="$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin"
root="$HOME/.stado/work"
/bin/mkdir -p "$root"
work=$(/usr/bin/mktemp -d "$root/release-platform.XXXXXX")
trap '/bin/rm -rf "$work"' EXIT HUP INT TERM
/usr/bin/git -C "$work" init -q source
/usr/bin/git -C "$work/source" remote add origin {repo}
/usr/bin/git -C "$work/source" fetch -q --depth 1 origin {revision}
/usr/bin/git -C "$work/source" checkout -q --detach FETCH_HEAD
/usr/bin/git clone -q --depth 1 https://github.com/wisent-ai/skarbiec.git "$work/skarbiec"
cargo build --release --manifest-path "$work/skarbiec/Cargo.toml"
export SKARBIEC_TEST_BIN="$work/skarbiec/target/release/skarbiec"
cd "$work/source/stado-rs"
cargo test --test builds build_recipe_polls_public_git_runs_on_matching_worker_and_publishes_artifact -- --ignored --nocapture --test-threads=1
cargo test --test ci-cd a_real_release_builds_publishes_and_installs_its_binary -- --ignored --nocapture --test-threads=1
"#
    );
    let output = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        &script,
        std::time::Duration::from_secs(45 * 60),
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        let detail = format!("{}\n{}", output.stdout, output.stderr);
        let tail = detail.lines().rev().take(80).collect::<Vec<_>>();
        return Err(CmdError::click(format!(
            "{target}: platform verification failed:\n{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        )));
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "target": target,
                "revision": revision.trim_matches('\''),
                "verified": true,
                "output": output.stdout,
            }))?
        );
    } else {
        print!("{}", output.stdout);
    }
    Ok(())
}

/// `stado host activate-staged-release TARGET --product P` — run the staged
/// release's OWN installer, once, on a host whose installed one cannot.
///
/// The host installs its own releases by running the installer that ships
/// inside the active release. When that copy is broken the host cannot install
/// the release that repairs it, and no amount of correct delivery reaches it:
/// charless-mac-mini sat with 0.5.43 staged in its local release root and an
/// activator logging a syntax error once a minute. This runs the staged copy
/// instead of the installed one. Nothing else changes - same env file, same
/// digest contract, same script.
pub async fn activate_staged_release(
    target: &str,
    product: &str,
    env_file: &str,
    port: u16,
    json: bool,
) -> Result<(), CmdError> {
    use crate::deploy::staged_release;

    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let click = |error: crate::deploy::DeployError| CmdError::click(error.to_string());

    let fetched = crate::deploy::service_file_fetch::fetch_file(&resolved, env_file, &runner)
        .await
        .map_err(click)?;
    if !fetched.ok() {
        return Err(CmdError::click(format!(
            "{}: could not read {env_file}: {}",
            resolved.name, fetched.report.file_state
        )));
    }
    let body = String::from_utf8_lossy(&fetched.content).into_owned();
    let coordinate = staged_release::coordinate(&body, product).map_err(click)?;

    let platform = crate::deploy::host_channel::run_script(
        &resolved,
        "set -eu\ncase \"$(uname -s)/$(uname -m)\" in\n  Darwin/arm64) echo darwin-arm64 ;;\n  \
         Darwin/x86_64) echo darwin-x64 ;;\n  Linux/aarch64) echo linux-arm64 ;;\n  \
         Linux/x86_64) echo linux-x64 ;;\n  *) echo unknown ;;\nesac\n",
        &runner,
    )
    .await
    .map_err(click)?;
    let platform = platform.stdout.trim().to_string();
    if platform == "unknown" {
        return Err(CmdError::click(format!(
            "{}: could not name this host's release platform",
            resolved.name
        )));
    }
    let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(click)?;
    let archive = staged_release::expand_home(
        &staged_release::archive_path(&coordinate, product, &platform),
        &home,
    )
    .map_err(click)?;

    // Refusal one: the staged bytes must be the bytes the coordinate declares.
    let hashed = crate::deploy::host_channel::run_script(
        &resolved,
        &format!(
            "set -eu\ntest -f {0} || {{ echo missing; exit 0; }}\nshasum -a 256 {0}\n",
            crate::deploy::shlex_quote(&archive)
        ),
        &runner,
    )
    .await
    .map_err(click)?;
    let Some(observed) = staged_release::parse_shasum(&hashed.stdout) else {
        return Err(CmdError::click(format!(
            "{}: no staged archive at {archive} to activate",
            resolved.name
        )));
    };
    staged_release::digest_verdict(&coordinate.sha256, observed).map_err(click)?;

    let before = staged_release::installed_version(&resolved, &runner)
        .await
        .map_err(click)?;
    let api_before = staged_release::api_answering(&resolved, port, &runner).await;
    if before == coordinate.version {
        if !json {
            println!(
                "{}: already running {product} {}; nothing to activate",
                resolved.name, coordinate.version
            );
        }
        return Ok(());
    }

    let outcome = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        &staged_release::activation_script(&archive, &coordinate.version),
        std::time::Duration::from_secs(900),
        &runner,
    )
    .await
    .map_err(click)?;

    let after = staged_release::installed_version(&resolved, &runner)
        .await
        .map_err(click)?;
    let api_after = staged_release::api_answering(&resolved, port, &runner).await;
    let log_tail = outcome
        .stdout
        .lines()
        .chain(outcome.stderr.lines())
        .filter(|line| line.contains("STADO_ACTIVATE"))
        .collect::<Vec<_>>()
        .join("\n");

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "host": resolved.name,
                "product": product,
                "declared": coordinate.version,
                "installed_before": before,
                "installed_after": after,
                "api_before": api_before,
                "api_after": api_after,
                "log": log_tail,
            }))?
        );
    } else {
        println!("host:      {}", resolved.name);
        println!("declared:  {product} {}", coordinate.version);
        println!("installed: {before} -> {after}");
        println!(
            "api:       {} -> {}",
            if api_before { "answering" } else { "silent" },
            if api_after { "answering" } else { "silent" }
        );
        if !log_tail.is_empty() {
            println!("{log_tail}");
        }
    }

    // The installer restarts what it activates; an API that was serving before
    // and is silent after is a failed activation, not a completed one.
    if api_before && !api_after {
        return Err(CmdError::click(format!(
            "{}: {product} {after} is installed but the API on {port} stopped answering; \
             it was answering before this ran",
            resolved.name
        )));
    }
    // `installed_version` reads the link, so a revert shows up as an unchanged
    // version even though the installer did everything right. The link either
    // side of the settle window is what separates the two.
    let activated = log_tail
        .lines()
        .find_map(|line| line.strip_prefix("STADO_ACTIVATE_LINK "))
        .unwrap_or("")
        .trim()
        .to_string();
    let settled = log_tail
        .lines()
        .find_map(|line| line.strip_prefix("STADO_ACTIVATE_SETTLED "))
        .unwrap_or("")
        .trim()
        .to_string();
    if !activated.is_empty() && activated != settled {
        return Err(CmdError::click(format!(
            "{}: the staged installer activated {activated}, and something on that host put the \
             runtime link back to {settled} within thirty seconds; the release is installed and \
             something else is holding the host on the old one",
            resolved.name
        )));
    }
    if after != coordinate.version {
        return Err(CmdError::click(format!(
            "{}: the staged installer ran but {product} is still {after}, not {}",
            resolved.name, coordinate.version
        )));
    }
    Ok(())
}
