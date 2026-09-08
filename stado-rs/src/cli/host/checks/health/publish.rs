use std::io::Read;

use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::host::checks::health::api::{host_health_api_token, host_health_api_url};
use crate::cli::host::checks::health::lifecycle::refresh_local_unit_lifecycle;

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
        let runner = crate::deploy::production_runner();
        refresh_local_unit_lifecycle(&mut document, &runner).await;
        let link = crate::deploy::host_link::collect_link(&runner).await;
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
