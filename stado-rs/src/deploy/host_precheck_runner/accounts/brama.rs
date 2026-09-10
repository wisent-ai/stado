//! The Brama the runner is permitted to dial, and the Skarbiec that Brama owns.

use crate::deploy::host_precheck_runner::verdict::report::command_failure;
use crate::deploy::{host_channel, production_runner, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The consumer identity the runner presents to Brama. It is the same name the
/// installer writes into `routes/kronika-agent-id`, so the authorization this
/// function checks and the identity the runner actually uses are one word.
const BRAMA_CONSUMER: &str = "kronika";

pub(crate) struct BramaSkarbiecContext {
    pub(crate) runner: Runner,
    pub(crate) skarbiec: String,
    pub(crate) vault: String,
    pub(crate) routes: String,
    pub(crate) gnupg: String,
    pub(crate) home: String,
}

fn brama_service_path(document: &str, key: &str, home: &str) -> Result<String, DeployError> {
    let prefix = format!("{key}=");
    let raw = document
        .lines()
        .map(str::trim)
        .map(|line| line.strip_prefix("export ").unwrap_or(line))
        .find_map(|line| line.strip_prefix(&prefix))
        .map(str::trim)
        .ok_or_else(|| DeployError(format!("Brama service environment has no {key}")))?;
    let value = if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        &raw[1..raw.len() - 1]
    } else {
        raw
    };
    let expanded = if let Some(rest) = value.strip_prefix("$HOME/") {
        format!("{home}/{rest}")
    } else if let Some(rest) = value.strip_prefix("${HOME}/") {
        format!("{home}/{rest}")
    } else if let Some(rest) = value.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else {
        value.to_string()
    };
    let home_prefix = format!("{home}/");
    let relative = expanded.strip_prefix(&home_prefix).ok_or_else(|| {
        DeployError(format!(
            "Brama service environment {key} must be an absolute path below the managed home"
        ))
    })?;
    if relative.is_empty()
        || relative
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || expanded.chars().any(char::is_control)
    {
        return Err(DeployError(format!(
            "Brama service environment {key} must be an absolute path below the managed home"
        )));
    }
    Ok(expanded)
}

pub(crate) async fn brama_skarbiec_context(
    target: &ComputeTarget,
) -> Result<BramaSkarbiecContext, DeployError> {
    let runner = production_runner();
    let home = host_channel::remote_home(target, &runner).await?;
    let service_env = format!("{home}/.config/brama/service.env");
    let service_paths = host_channel::run_program(
        target,
        &[
            "/usr/bin/grep",
            "-E",
            "^(export )?(SKARBIEC_(VAULT_FILE|CAPABILITY_ROUTES_FILE)|BRAMA_GNUPG_HOME)=",
            &service_env,
        ],
        &runner,
    )
    .await?;
    if !service_paths.ok() {
        return Err(DeployError(format!(
            "{}: cannot read Brama's Skarbiec path declarations: {}",
            target.name,
            command_failure(
                &service_paths,
                "Brama service environment path lookup failed"
            )
        )));
    }
    let vault_path = brama_service_path(&service_paths.stdout, "SKARBIEC_VAULT_FILE", &home)?;
    let routes_path = brama_service_path(
        &service_paths.stdout,
        "SKARBIEC_CAPABILITY_ROUTES_FILE",
        &home,
    )?;
    let gnupg_path = brama_service_path(&service_paths.stdout, "BRAMA_GNUPG_HOME", &home)?;
    Ok(BramaSkarbiecContext {
        runner,
        skarbiec: format!("{home}/.stado/bin/skarbiec"),
        vault: format!("SKARBIEC_VAULT_FILE={vault_path}"),
        routes: format!("SKARBIEC_CAPABILITY_ROUTES_FILE={routes_path}"),
        gnupg: format!("GNUPGHOME={gnupg_path}"),
        home,
    })
}

/// The host whose Brama installation holds the Probierz agent identity for a
/// runner being installed on `target`.
///
/// The runner's own box when Brama runs there, and Brama's single active host
/// otherwise. The Probierz identity is not a per-machine secret: it is one fleet
/// agent identity, minted by the capability-routes table of the Brama the runner
/// is authorized against. Reading it beside that Brama is the only place it can
/// be read from without keeping a second copy in a second vault, and a host that
/// does not run Brama has no such vault at all -- which is what
/// `cannot read Brama's Skarbiec path declarations:
/// /root/.config/brama/service.env: No such file or directory` was reporting.
///
/// The secret still lands only in the runner host's own owner-only file, written
/// by [`super::install::install_kronika_agent_secret`] through the audited
/// channel.
pub(crate) async fn brama_identity_host(
    target: &ComputeTarget,
) -> Result<ComputeTarget, DeployError> {
    let registry = host_channel::canonical_registry().await?;
    let service = registry
        .service("brama")
        .ok_or_else(|| DeployError("service directory carries no brama service".to_string()))?;
    if service.active_host == target.name {
        return Ok(target.clone());
    }
    host_channel::resolve_target(&registry, &service.active_host).cloned()
}

/// The loopback origin the runner's egress boundary opens, and the port that
/// boundary permits.
///
/// Only a private loopback HTTP origin can be returned, because that is the
/// only thing the rendered boundary can express: the runner uid is permitted
/// exactly `127.0.0.1:<port>` and rejected on every network in
/// [`crate::deploy::host_precheck_runner::platform::BLOCKED_IPV4_NETWORKS`] and
/// [`crate::deploy::host_precheck_runner::platform::BLOCKED_IPV6_NETWORKS`], which includes the tailnet and
/// the rest of loopback. The route has to terminate on the runner's own box.
///
/// Where that origin is declared depends on whether the box runs Brama, and
/// this used to read one map for both cases -- which is why it refused every
/// host except the one Brama is active on, with `brama declares no endpoint for
/// runner target ...`.
///
/// [`crate::targets::Service::endpoints`] is "the address a host DIALS", and
/// the directory may only carry an entry where something genuinely answers it.
/// Three rules enforce that, and together they mean the map can name the box
/// Brama runs on and the boxes it could move to, and no third machine that
/// merely calls it:
///
/// * `stado service verify` probes each entry from the host it names and
///   reports silence as `unreachable`, exiting non-zero.
/// * `brama` is a placement-backed route (`brama-skarbiec`), so
///   [`crate::service_resolution::validate_registry_contract`] requires
///   `endpoints` and `standby` together to name exactly that profile's hosts.
///   An entry for any other target makes the whole registry document invalid
///   for every reader on the fleet.
/// * within that profile, `active_profile_host` refuses a document in which
///   more than one host declares the managed unit. There is one Brama.
///
/// So a runner host that is not Brama's active host reaches Brama the way every
/// other consumer on such a host does: through that host's own resolver
/// adapter, which binds a loopback port and proxies to the active host. That
/// socket must never be written into the directory --
/// [`crate::service_resolution::self_referencing_endpoints`] reports exactly
/// that shape and `stado registry doctor` exits non-zero on it -- so it is read
/// from where it IS declared: the target's own `service_resolver` policy.
///
/// Either way the runner dials Brama and nothing else. Nothing here can name a
/// provider, and nothing here can name a host other than the runner's own.
pub(crate) async fn private_brama_route(target_name: &str) -> Result<(String, u16), DeployError> {
    let registry = host_channel::canonical_registry().await?;
    let service = registry
        .service("brama")
        .ok_or_else(|| DeployError("service directory carries no brama service".to_string()))?;
    let consumer = service.consumers.get(BRAMA_CONSUMER).ok_or_else(|| {
        DeployError(format!(
            "brama does not authorize consumer {BRAMA_CONSUMER:?}"
        ))
    })?;
    if !consumer
        .capabilities
        .iter()
        .any(|capability| capability == "model-routing")
    {
        return Err(DeployError(format!(
            "brama consumer {BRAMA_CONSUMER:?} lacks model-routing"
        )));
    }
    let url = if service.active_host == target_name {
        // The box serves Brama itself, so the address it dials is the address
        // it serves on, and the directory is the one place that records it.
        service
            .address_for(target_name)
            .map(|endpoint| endpoint.url.clone())
            .ok_or_else(|| {
                DeployError(format!(
                    "brama is active on {target_name:?} and the directory declares no endpoint there"
                ))
            })?
    } else {
        brama_gateway_origin(&registry, target_name)?
    };
    let parsed = url::Url::parse(&url)
        .map_err(|error| DeployError(format!("brama route is invalid: {error}")))?;
    if parsed.scheme() != "http"
        || !matches!(parsed.host_str(), Some("127.0.0.1" | "localhost"))
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(DeployError(format!(
            "brama route for {target_name:?} must be a private loopback HTTP origin, got {url}"
        )));
    }
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| DeployError("brama route has no port".to_string()))?;
    Ok((url.trim_end_matches('/').to_string(), port))
}

/// The loopback socket `target_name`'s own resolver publishes for Brama to the
/// runner's consumer identity, as an HTTP origin.
///
/// Read from the registry rather than probed, for the same reason the rest of
/// this module reads the registry: the installer renders a boundary from it, and
/// a boundary rendered from an observation is a boundary that changes when the
/// observation was taken. Whether the socket is answering is
/// `stado resolver status` on that host, and the installed runner's own
/// `precheck-runner status`.
fn brama_gateway_origin(
    registry: &crate::targets::Registry,
    target_name: &str,
) -> Result<String, DeployError> {
    let target = host_channel::resolve_target(registry, target_name)?;
    let declared = target.extra.get("service_resolver").ok_or_else(|| {
        DeployError(format!(
            "brama is not active on {target_name:?} and that target declares no service_resolver, \
             so nothing on it publishes a Brama route the runner could be permitted to dial"
        ))
    })?;
    let config: crate::service_resolution::ResolverConfig =
        serde_json::from_value(declared.clone()).map_err(|error| {
            DeployError(format!(
                "{target_name}: registry target service_resolver is invalid: {error}"
            ))
        })?;
    let adapter = config
        .adapters
        .iter()
        .find(|adapter| adapter.service == "brama" && adapter.consumer == BRAMA_CONSUMER)
        .ok_or_else(|| {
            DeployError(format!(
                "{target_name}: its resolver declares no brama adapter for consumer \
                 {BRAMA_CONSUMER:?}, so the runner has no Brama route on its own host"
            ))
        })?;
    Ok(format!("http://{}", adapter.bind))
}
