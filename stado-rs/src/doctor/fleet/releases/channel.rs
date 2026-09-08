//! The exact release coordinate this deployment runs, and the route it
//! travels.

use std::time::Duration;

use serde_json::Value;

use crate::config;
use crate::doctor::{Check, Findings, Status, LOCAL_PROVIDER};

// ---------------------------------------------------------------------------
// 5. Release channel
// ---------------------------------------------------------------------------

pub(in crate::doctor) const RELEASE_ID: &str = "release";
pub(in crate::doctor) const RELEASE_TITLE: &str = "Release channel";
pub(in crate::doctor) const RELEASE_REMEDY: &str =
    "set canonical STADO_API_URL plus exact STADO_RELEASE_VERSION and \
     STADO_RELEASE_PLATFORM (config keys api.url, release.version, and release.platform); publish \
     the canonical archive and manifest for every supported platform";

/// GET the exact release checksum manifest through the same public Stado route
/// used by agent startup. A missing coordinate, route failure, malformed
/// manifest, or absent binary checksum is a hard failure before dispatch.
pub(in crate::doctor) async fn check_release_channel() -> Check {
    let api = config::stado_api_url();
    let version = config::stado_release_version();
    let platform = config::stado_release_platform();
    let local_only = config::wc_providers()
        .iter()
        .all(|provider| provider == LOCAL_PROVIDER);
    if local_only && api.is_empty() && version.is_empty() && platform.is_empty() {
        return Check::pass(
            RELEASE_ID,
            RELEASE_TITLE,
            "local-only outage profile uses the installed Rust binary; no cloud VM release is active"
                .to_string(),
            RELEASE_REMEDY,
        );
    }

    let mut findings = Findings::default();
    if !api.starts_with("https://") || version.is_empty() || platform.is_empty() {
        findings.note(
            Status::Fail,
            "the public release API, exact version, and exact platform must all be configured"
                .to_string(),
        );
        findings.remedy(RELEASE_REMEDY);
        return findings.into_check(RELEASE_ID, RELEASE_TITLE, RELEASE_REMEDY);
    }

    // A 200 for a 201-byte manifest proves the name answers. It does not prove
    // the route the release archive will travel, and those came apart on
    // 2026-09-02: this check passed while the same origin resolved to the
    // public `ts.net` front end and a release train moved 20 MB per 55 seconds
    // until it was cancelled. Ask where the origin actually is before asking
    // what it serves.
    findings_for_origin_route(&api, &mut findings).await;

    let uri =
        format!("stado://releases/stado/{version}/{platform}/release-manifest-{platform}.json");
    let endpoint = format!("{api}/api/release/object");
    // The client the product itself uses, trust store, timeouts, tailnet route
    // and all. A bare `reqwest::Client::new()` here measured a different path
    // than the one a release travels, which is how this check kept passing
    // over a route no release could use.
    let client = match crate::cli::storage::fleet_https_client() {
        Ok(client) => client,
        Err(error) => {
            findings.note(
                Status::Fail,
                format!("cannot build the fleet HTTPS client: {error}"),
            );
            findings.remedy(RELEASE_REMEDY);
            return findings.into_check(RELEASE_ID, RELEASE_TITLE, RELEASE_REMEDY);
        }
    };
    let response = match client
        .get(&endpoint)
        .query(&[("uri", uri.as_str())])
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            findings.note(
                Status::Fail,
                format!("exact release manifest {uri} is unreachable: {error}"),
            );
            findings.remedy(RELEASE_REMEDY);
            return findings.into_check(RELEASE_ID, RELEASE_TITLE, RELEASE_REMEDY);
        }
    };
    if !response.status().is_success() {
        findings.note(
            Status::Fail,
            format!(
                "exact release manifest {uri} returned HTTP {}",
                response.status()
            ),
        );
        findings.remedy(RELEASE_REMEDY);
        return findings.into_check(RELEASE_ID, RELEASE_TITLE, RELEASE_REMEDY);
    }
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) => {
            findings.note(
                Status::Fail,
                format!("cannot read exact release manifest {uri}: {error}"),
            );
            findings.remedy(RELEASE_REMEDY);
            return findings.into_check(RELEASE_ID, RELEASE_TITLE, RELEASE_REMEDY);
        }
    };
    match serde_json::from_str::<Value>(&body) {
        Ok(manifest)
            if manifest.get("product").and_then(Value::as_str) == Some("stado")
                && manifest.get("version").and_then(Value::as_str) == Some(version.as_str())
                && manifest.get("platform").and_then(Value::as_str) == Some(platform.as_str())
                && manifest.as_object().is_some_and(|object| object.len() == 5)
                && manifest
                    .get("source_commit")
                    .and_then(Value::as_str)
                    .is_some_and(|commit| {
                        matches!(commit.len(), 40 | 64)
                            && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                && manifest
                    .get("sha256")
                    .and_then(Value::as_str)
                    .is_some_and(|digest| {
                        digest.len() == 64
                            && digest
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    }) =>
        {
            findings.note(
                Status::Pass,
                format!("{uri} is reachable and identifies the immutable release archive"),
            )
        }
        Ok(_) => {
            findings.note(
                Status::Fail,
                format!("{uri} has invalid identity or archive digest"),
            );
            findings.remedy(RELEASE_REMEDY);
        }
        Err(error) => {
            findings.note(
                Status::Fail,
                format!("{uri} is not a valid release manifest: {error}"),
            );
            findings.remedy(RELEASE_REMEDY);
        }
    }
    findings.into_check(RELEASE_ID, RELEASE_TITLE, RELEASE_REMEDY)
}

/// Where a tailnet release origin resolves, judged against the tailnet's own
/// map of its names.
///
/// Stado pins the tailnet route itself, so a disagreement is a warning rather
/// than a failure: this process will reach the right address, and the host
/// scripts that fetch a release with `curl` will not.
async fn findings_for_origin_route(api: &str, findings: &mut Findings) {
    let Some(host) = url::Url::parse(api)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .filter(|host| crate::tailnet::is_magicdns_name(host))
    else {
        return;
    };
    let pinned = crate::tailnet::address_of(&host);
    // A resolver with no answer for this suffix is exactly the state being
    // measured, and `getaddrinfo` can sit on one of those for seconds. Bound
    // it well inside the shared probe deadline: no answer in two seconds is
    // the answer.
    let resolved: Vec<std::net::IpAddr> = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::lookup_host(format!("{host}:443")),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .map(|addresses| addresses.map(|address| address.ip()).collect())
    .unwrap_or_default();
    match pinned {
        None if resolved.is_empty() => {
            findings.note(
                Status::Fail,
                format!(
                    "{host} is a tailnet name that neither this node's tailnet map nor the \
                     system resolver can place"
                ),
            );
            findings.remedy(ORIGIN_ROUTE_REMEDY);
        }
        None => findings.note(
            Status::Warn,
            format!(
                "{host} resolves only through the system resolver ({}); this node's tailnet map \
                 does not name it, so the route is whatever public DNS answers",
                render_addresses(&resolved)
            ),
        ),
        Some(address) if !resolved.contains(&address) => {
            findings.note(
                Status::Warn,
                format!(
                    "the tailnet places {host} at {address}; the system resolver answers {}. \
                     Stado pins the tailnet route, a host-side curl does not",
                    render_addresses(&resolved)
                ),
            );
            findings.remedy(ORIGIN_ROUTE_REMEDY);
        }
        Some(address) => findings.note(
            Status::Pass,
            format!("{host} resolves to its tailnet address {address}"),
        ),
    }
}

const ORIGIN_ROUTE_REMEDY: &str =
    "make the tailnet's MagicDNS suffix resolve through the tailnet responder (100.100.100.100) \
     on this machine, so every consumer of the release origin takes the tailnet route and not a \
     public front end";

fn render_addresses(addresses: &[std::net::IpAddr]) -> String {
    if addresses.is_empty() {
        return "nothing".to_string();
    }
    addresses
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
