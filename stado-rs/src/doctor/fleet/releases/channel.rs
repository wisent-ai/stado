//! The exact release coordinate this deployment runs, and the route it
//! travels.

use std::net::IpAddr;

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
     that version with `stado release submit` for every supported platform";

/// GET the exact signed release manifest (`release.json`) through the same
/// public Stado route agent startup and the installer use. A missing
/// coordinate, route failure, malformed manifest, or invalid archive digest is
/// a hard failure before dispatch.
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
    // the route the release archive will travel: this check can pass while
    // the same origin resolves to the public `ts.net` front end and a release
    // train crawls at kilobytes per second. Ask where the origin actually is
    // before asking what it serves.
    findings_for_origin_route(&api, &mut findings).await;

    let uri = format!("stado://releases/stado/{version}/{platform}/release.json");
    let verdict = match fetch_manifest(&api, &uri).await {
        Ok(manifest) => manifest_identity(&manifest, &version, &platform)
            .map_err(|refusal| format!("{uri}: {refusal}")),
        Err(refusal) => Err(refusal),
    };
    match verdict {
        Ok(()) => findings.note(
            Status::Pass,
            format!("{uri} is reachable and identifies the immutable release archive"),
        ),
        Err(refusal) => {
            findings.note(Status::Fail, refusal);
            findings.remedy(RELEASE_REMEDY);
        }
    }
    findings.into_check(RELEASE_ID, RELEASE_TITLE, RELEASE_REMEDY)
}

/// The manifest at `uri`, read through the client the product itself uses,
/// trust store, tailnet route and all, or why it could not be read. A bare
/// `reqwest::Client::new()` here measured a different path than the one a
/// release travels, which is how this check kept passing over a route no
/// release could use.
async fn fetch_manifest(api: &str, uri: &str) -> Result<Value, String> {
    let client = crate::cli::storage::fleet_https_client()
        .map_err(|error| format!("cannot build the fleet HTTPS client: {error}"))?;
    let endpoint = format!("{api}/api/release/object");
    let response = crate::wait::request(client.get(&endpoint).query(&[("uri", uri)]))
        .await
        .map_err(|error| format!("exact release manifest {uri} is unreachable: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "exact release manifest {uri} returned HTTP {}",
            response.status()
        ));
    }
    let body = response
        .text()
        .await
        .map_err(|error| format!("cannot read exact release manifest {uri}: {error}"))?;
    serde_json::from_str(&body)
        .map_err(|error| format!("{uri} is not a valid release manifest: {error}"))
}

/// Whether `manifest` names this deployment's product, version and platform,
/// a full source commit and a SHA-256 archive digest; the first field that
/// does not, with what it holds, when one does not.
fn manifest_identity(manifest: &Value, version: &str, platform: &str) -> Result<(), String> {
    let field = |name: &str| manifest.get(name).and_then(Value::as_str);
    for (name, expected) in [
        ("product", "stado"),
        ("version", version),
        ("platform", platform),
    ] {
        match field(name) {
            Some(stated) if stated == expected => {}
            Some(stated) => return Err(format!("its {name} is {stated:?}, not {expected:?}")),
            None => return Err(format!("it states no {name}")),
        }
    }
    // A git object id: SHA-1, or SHA-256 in a repository of that format.
    let commit = field("source_revision").ok_or("it states no source_revision")?;
    if !crate::binary::provenance::is_commit_id(commit)
        && !crate::deploy::host_release::is_sha256(commit)
    {
        return Err(format!(
            "its source_revision {commit:?} is not a full git object id"
        ));
    }
    let digest = field("artifact_sha256").ok_or("it states no artifact_sha256")?;
    if !crate::deploy::host_release::is_sha256(digest) {
        return Err(format!(
            "its artifact_sha256 {digest:?} is not a lowercase SHA-256"
        ));
    }
    Ok(())
}

/// Where a tailnet release origin resolves, judged against the tailnet's own
/// map of its names.
///
/// Stado pins the tailnet route itself, so a disagreement is a warning rather
/// than a failure: this process will reach the right address, and the host
/// scripts that fetch a release with `curl` will not.
async fn findings_for_origin_route(api: &str, findings: &mut Findings) {
    let origin = match url::Url::parse(api) {
        Ok(origin) => origin,
        Err(error) => {
            findings.note(
                Status::Fail,
                format!("STADO_API_URL {api:?} is not a URL: {error}"),
            );
            findings.remedy(RELEASE_REMEDY);
            return;
        }
    };
    let Some(host) = origin
        .host_str()
        .filter(|host| crate::remote::tailnet::is_magicdns_name(host))
        .map(str::to_string)
    else {
        return;
    };
    // The port the release travels to: the URL's own, or its scheme's.
    let Some(port) = origin.port_or_known_default() else {
        findings.note(
            Status::Fail,
            format!("STADO_API_URL {api:?} names no port and no known scheme"),
        );
        findings.remedy(RELEASE_REMEDY);
        return;
    };
    let pinned = crate::remote::tailnet::address_of(&host);
    // A resolver with no answer for this suffix is exactly the state being
    // measured; its own failure is the answer, and it is said as it came.
    let resolved: Result<Vec<IpAddr>, String> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map(|addresses| addresses.map(|address| address.ip()).collect())
        .map_err(|error| error.to_string());
    match (pinned, resolved) {
        (None, Err(said)) => {
            findings.note(
                Status::Fail,
                format!(
                    "{host} is a tailnet name that neither this node's tailnet map nor the \
                     system resolver can place (the resolver said: {said})"
                ),
            );
            findings.remedy(ORIGIN_ROUTE_REMEDY);
        }
        (None, Ok(resolved)) if resolved.is_empty() => {
            findings.note(
                Status::Fail,
                format!(
                    "{host} is a tailnet name that neither this node's tailnet map nor the \
                     system resolver can place (the resolver answered no address)"
                ),
            );
            findings.remedy(ORIGIN_ROUTE_REMEDY);
        }
        (None, Ok(resolved)) => findings.note(
            Status::Warn,
            format!(
                "{host} resolves only through the system resolver ({}); this node's tailnet map \
                 does not name it, so the route is whatever public DNS answers",
                render_addresses(&resolved)
            ),
        ),
        (Some(address), Err(said)) => {
            findings.note(
                Status::Warn,
                format!(
                    "the tailnet places {host} at {address}; the system resolver cannot place it \
                     ({said}). Stado pins the tailnet route, a host-side curl does not"
                ),
            );
            findings.remedy(ORIGIN_ROUTE_REMEDY);
        }
        (Some(address), Ok(resolved)) if !resolved.contains(&address) => {
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
        (Some(address), Ok(_)) => findings.note(
            Status::Pass,
            format!("{host} resolves to its tailnet address {address}"),
        ),
    }
}

const ORIGIN_ROUTE_REMEDY: &str =
    "make the tailnet's MagicDNS suffix resolve through the tailnet's own DNS responder on this \
     machine (Tailscale's `--accept-dns`), so every consumer of the release origin takes the \
     tailnet route and not a public front end";

fn render_addresses(addresses: &[IpAddr]) -> String {
    if addresses.is_empty() {
        return "nothing".to_string();
    }
    addresses
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}
