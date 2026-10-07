use super::super::catalog_identity;
use super::refusals::{is_exact_semver, loopback_http_origin};
use super::ReleaseRequest;
use crate::deploy::products;
use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The service-directory name of the fleet object API. The host the
/// directory says serves it is the one host a loopback release origin is
/// self-delivery for.
const OBJECT_API_SERVICE: &str = "stado-object-api";

/// The route a target reads a release object through.
const RELEASE_ROUTE: &str = "/api/release/object";

/// A loopback release origin on a remote host is trusted only when
/// `stado route open` recorded that exact directory route endpoint there. The
/// marker is not proof that the tunnel is still alive—the subsequent fetch
/// proves that—but it proves loopback HTTP names an encrypted Stado-managed
/// rather than an undeclared clear-text transport.
async fn has_managed_loopback_forward(
    target: &ComputeTarget,
    endpoint: &str,
    runner: &Runner,
) -> Result<bool, DeployError> {
    if !loopback_http_origin(endpoint) {
        return Ok(false);
    }
    let script = format!(
        "set -u\n\
         endpoint={}\n\
         directory=\"$HOME/.stado/forwards\"\n\
         [ -d \"$directory\" ] || exit 0\n\
         for marker in \"$directory\"/*.url; do\n\
           [ -f \"$marker\" ] && [ ! -L \"$marker\" ] || continue\n\
           value=$(/usr/bin/head -n 1 \"$marker\" 2>/dev/null || true)\n\
           if [ \"$value\" = \"$endpoint\" ]; then\n\
             printf 'STADO_RELEASE_FORWARD\\t%s\\n' \"$marker\"\n\
             exit 0\n\
           fi\n\
         done\n",
        shlex_quote(endpoint)
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(output.ok()
        && output
            .stdout
            .lines()
            .any(|line| line.starts_with("STADO_RELEASE_FORWARD\t")))
}

/// Resolve one declared immutable release while its catalog authority is
/// available. Storage recovery persists this request before taking that
/// authority offline, then uses the ordinary stage and activation programs;
/// no transaction-specific executable path enters a unit file.
pub async fn resolve_release_request(
    target_name: &str,
    binary: &str,
    version: &str,
    dry_run: bool,
    reinstall: bool,
    runner: &Runner,
) -> Result<(ComputeTarget, ReleaseRequest, bool), DeployError> {
    let product = products::product(binary)?;
    if !is_exact_semver(version) {
        return Err(DeployError(format!(
            "{version:?} is not an exact version; --version takes a semantic version such as \
             0.5.1, never a channel, an alias or a range. A release coordinate is immutable"
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(DeployError::from)?;
    let target = host_channel::resolve_target(&registry, target_name)?.clone();
    let platform = products::managed_platform(&target.release_platform)?;
    product.platform(platform)?;
    // The origin the registry declares for public release reads is the one a
    // target can reach without a bearer; the configured API origin answers
    // only while an edge still forwards the route, and an edge can stop. A
    // fleet that declares no such origin
    // keeps its configured one.
    let declared = serde_json::Value::Object(registry.extra.clone());
    let release_api = match crate::public_origin::publishing(&declared, RELEASE_ROUTE) {
        Ok(origin) => origin.origin(),
        Err(_) if crate::public_origin::declarations(&declared).is_empty() => {
            crate::cli::storage::release_api_origin().map_err(DeployError::from)?
        }
        Err(refusal) => {
            return Err(
                DeployError(refusal).stating(crate::primitives::failure::FailureCode::Config)
            )
        }
    };
    let identity = catalog_identity(product, version, platform).await?;
    let local_target = registry
        .lookup_self(&crate::providers::vast::system_hostname())
        .map_err(DeployError::from)?
        .is_some_and(|local| local.name == target.name);
    let managed_loopback = has_managed_loopback_forward(&target, &release_api, runner).await?;
    let self_store = local_target
        || managed_loopback
        || registry
            .service(OBJECT_API_SERVICE)
            .is_some_and(|object_api| object_api.active_host == target.name);
    let target_release_api = registry
        .service(OBJECT_API_SERVICE)
        .filter(|object_api| object_api.active_host == target.name)
        .and_then(|object_api| object_api.address_for(&target.name))
        .map(|endpoint| endpoint.url.trim_end_matches('/').to_string())
        .filter(|url| loopback_http_origin(url))
        .unwrap_or(release_api);
    let archive_uri = format!(
        "stado://releases/{}/{version}/{platform}/{}",
        product.source.product, identity.archive_name
    );
    let archive_bytes = crate::cli::storage::release_object_size(&archive_uri)
        .await
        .map_err(DeployError::from)?;
    let request = ReleaseRequest {
        binary: product.name.clone(),
        version: version.to_string(),
        platform: platform.to_string(),
        source_commit: identity.source_commit,
        sha256: identity.sha256,
        archive_name: identity.archive_name,
        member: identity.member,
        archive_bytes,
        release_api: target_release_api,
        dry_run,
        reinstall,
    };
    Ok((target, request, self_store))
}
