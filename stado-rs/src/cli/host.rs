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
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?
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
    let expected_path = format!("{}/{host}.json", crate::monitor::host_health::HEALTH_PREFIX);
    if payload
        != json!({
            "state": "stored",
            "host": host,
            "path": expected_path,
        })
    {
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

async fn host_health_api_token() -> Result<String, CmdError> {
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
    let item = client
        .read_item("stado-host-health-api")
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let token = item
        .get("token")
        .and_then(Value::as_str)
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
/// Transfer one opaque owner credential without exposing it in argv, stdout,
/// logs, a remote environment variable, or a general-purpose remote shell.
pub async fn install_secret(
    target: &str,
    source: &str,
    name: &str,
    json: bool,
) -> Result<(), CmdError> {
    release_component("secret file name", name)?;
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
    let bytes = std::fs::read(source)?;
    if bytes.is_empty() || bytes.len() > usize::from(u16::MAX) {
        return Err(CmdError::click(
            "host secret must contain between one and 65535 bytes",
        ));
    }
    let mut digest = Sha256::new();
    digest.update(&bytes);
    let expected_sha256 = hex::encode(digest.finalize());
    let payload = STANDARD.encode(&bytes);
    let remote_name = crate::deploy::shlex_quote(name);
    let remote_expected = crate::deploy::shlex_quote(&expected_sha256);
    let script = format!(
        r#"set -euo pipefail
name={remote_name}
expected={remote_expected}
case "$name" in
  ""|*[!A-Za-z0-9._-]*) printf '%s\n' 'invalid secret file name' >&2; exit 1 ;;
esac
dir="$HOME/.stado"
tmp="$dir/.${{name}}.stado-secret.$$"
trap 'rm -f "$tmp"' EXIT
/bin/mkdir -p "$dir"
/bin/chmod 700 "$dir"
if [ "$(/usr/bin/uname -s)" = "Darwin" ]; then decode=-D; else decode=--decode; fi
printf '%s' '{payload}' | /usr/bin/base64 "$decode" > "$tmp"
/bin/chmod 600 "$tmp"
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
    let path = format!("$HOME/.stado/{name}");
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "source": source,
                "path": path,
                "bytes": bytes.len(),
                "integrity": "sha256",
                "status": "installed",
            }))?
        );
    } else {
        println!(
            "{target}: installed owner-only {path} ({} bytes)",
            bytes.len()
        );
    }
    Ok(())
}

/// Run one helper previously placed in the remote owner-only Stado directory.
/// No arguments are accepted: the helper is the reviewed deployment program,
/// not an arbitrary shell escape.
pub async fn run_helper(target: &str, name: &str, json: bool) -> Result<(), CmdError> {
    release_component("helper name", name)?;
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let remote_name = crate::deploy::shlex_quote(name);
    let script = format!(
        r#"set -euo pipefail
helper="$HOME/.stado/bin/"{remote_name}
if [ ! -f "$helper" ] || [ -L "$helper" ] || [ ! -x "$helper" ]; then
  printf '%s\n' "missing executable regular Stado helper: $helper" > /dev/stderr
  false
fi
exec "$helper"
"#
    );
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
