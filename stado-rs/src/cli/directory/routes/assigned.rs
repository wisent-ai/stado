//! The port a catalog service listens on, on one host.
//!
//! The catalog used to carry each service's port, numbers an agent picked
//! once; one of them was later taken by another product and the two fought
//! over the socket. A port is now a fact about one host and lives where the
//! fleet already looks for addresses: the service directory's endpoint for
//! that host. When the directory records none, the host's own system hands
//! out a free loopback port (`stado host free-port-local`) and it is recorded
//! there before the unit is rendered, so the same port survives every later
//! ensure and every consumer's forward marker names it.

use serde_json::{json, Value};

use crate::cli::directory::document::DIRECTORY_KEY;
use crate::cli::directory::routes::service_port;
use crate::cli::registry;
use crate::cli::CmdError;
use crate::deploy::host_channel;
use crate::deploy::service_catalog::CatalogService;
use crate::targets::ComputeTarget;

/// The word a catalog entry writes where its port goes.
const PORT_PLACEHOLDER: &str = "$STADO_LISTEN_PORT";

/// Whether the entry's program, arguments or environment take a port.
fn names_port(entry: &CatalogService) -> bool {
    entry.program.contains(PORT_PLACEHOLDER)
        || entry.args.iter().any(|arg| arg.contains(PORT_PLACEHOLDER))
        || entry
            .env
            .values()
            .any(|value| value.contains(PORT_PLACEHOLDER))
}

/// The key the service directory files this catalog service under.
fn directory_name(entry: &CatalogService) -> &str {
    entry.directory_service.as_deref().unwrap_or(&entry.name)
}

/// The port one directory entry records for `target`, when it records an
/// endpoint for that host at all.
fn recorded_on(entry: &Value, target: &str) -> Option<u16> {
    entry.get("endpoints")?.get(target)?;
    service_port(entry, target)
}

/// Every port the directory records for `target`, with the service it
/// belongs to.
fn ports_on(document: &Value, target: &str) -> Vec<(String, u16)> {
    document
        .get(DIRECTORY_KEY)
        .and_then(|block| block.get("services"))
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, entry)| recorded_on(entry, target).map(|port| (name.clone(), port)))
        .collect()
}

/// The port the directory records for `entry` on `target`, without assigning
/// one. A catalog entry that takes no port answers `None`.
pub(crate) async fn recorded_listen_port(
    entry: &CatalogService,
    target: &str,
) -> Result<Option<u16>, CmdError> {
    if !names_port(entry) {
        return Ok(None);
    }
    let document = registry::fetch_document().await?;
    let service = directory_name(entry);
    Ok(document
        .get(DIRECTORY_KEY)
        .and_then(|block| block.get("services"))
        .and_then(|services| services.get(service))
        .and_then(|recorded| recorded_on(recorded, target)))
}

/// A free loopback port the host's own system hands out. An installed Stado
/// too old to know the verb is refused with the command that updates it.
pub(crate) async fn host_free_port(
    target: &ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<u16, CmdError> {
    let home = host_channel::remote_home(target, runner)
        .await
        .map_err(CmdError::from)?;
    let program = format!("{home}/.stado/bin/stado");
    let output = host_channel::run_program(
        target,
        &[program.as_str(), "host", "free-port-local"],
        runner,
    )
    .await
    .map_err(CmdError::from)?;
    let printed = output.stdout.trim();
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: the host could not hand out a free port ({}); install the current Stado there \
             with `stado release version converge --host {}`",
            target.name,
            host_channel::last_error_line(&output, "the command printed nothing"),
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    printed.parse::<u16>().map_err(|_| {
        CmdError::click(format!(
            "{}: `stado host free-port-local` printed {printed:?}, not a port",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}

/// Record `port` as `service`'s endpoint for `target`, creating the
/// directory entry when the service has none, under the block's generation.
async fn record_port(service: &str, target: &str, port: u16) -> Result<(), CmdError> {
    registry::commit_document(|current| {
        let mut document = current.clone();
        if let Some((other, _)) = ports_on(&document, target)
            .into_iter()
            .find(|(name, taken)| *taken == port && name != service)
        {
            return Err(CmdError::refused(format!(
                "{target}: the host handed out port {port}, which the service directory already \
                 records for {other} there; run the ensure again for another port"
            )));
        }
        {
            let services = document
                .get_mut(DIRECTORY_KEY)
                .and_then(Value::as_object_mut)
                .ok_or_else(|| {
                    CmdError::declaration(format!(
                        "the registry has no {DIRECTORY_KEY} block to record {service}'s port in"
                    ))
                })?
                .entry("services")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| {
                    CmdError::declaration(format!("{DIRECTORY_KEY}.services: must be an object"))
                })?;
            let entry = services
                .entry(service.to_string())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| {
                    CmdError::declaration(format!(
                        "{DIRECTORY_KEY}.services.{service}: must be an object"
                    ))
                })?;
            entry.entry("active_host").or_insert_with(|| json!(target));
            let endpoints = entry
                .entry("endpoints")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| {
                    CmdError::declaration(format!(
                        "{DIRECTORY_KEY}.services.{service}.endpoints: must be an object"
                    ))
                })?;
            endpoints.insert(
                target.to_string(),
                json!({ "url": format!("http://{}:{port}", std::net::Ipv4Addr::LOCALHOST) }),
            );
        }
        crate::service_resolution::advance_generation(&mut document)
            .map_err(CmdError::declaration)?;
        Ok(document)
    })
    .await?;
    Ok(())
}

/// The port `entry` listens on when it runs on `target`: the one the service
/// directory records for that host, or a free one the host hands out, which
/// is recorded before this returns. A catalog entry that takes no port
/// answers `None`.
pub(crate) async fn listen_port_for(
    entry: &CatalogService,
    target: &ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<Option<u16>, CmdError> {
    if let Some(port) = recorded_listen_port(entry, &target.name).await? {
        return Ok(Some(port));
    }
    if !names_port(entry) {
        return Ok(None);
    }
    let service = directory_name(entry);
    let port = host_free_port(target, runner).await?;
    record_port(service, &target.name, port).await?;
    eprintln!(
        "{}: the service directory recorded no port for {service}; the host handed out {port} \
         and it is recorded as {service}'s endpoint there",
        target.name
    );
    Ok(Some(port))
}
