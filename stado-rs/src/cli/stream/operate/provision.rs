//! Reconciling a host to its declaration, and recording the two services that
//! reconciliation leaves behind in the canonical registry.

use serde_json::Value;

use crate::cli::registry::{fetch_versioned_document, push_document_if};
use crate::cli::stream::report::{click, emitted, field};
use crate::cli::CmdError;
use crate::deploy::{production_runner, service, stream as remote};
use crate::stream::schema::DisplayStream;

fn declaration_of(target: &crate::targets::ComputeTarget) -> Result<DisplayStream, CmdError> {
    target.display_stream.clone().ok_or_else(|| {
        CmdError::click(format!(
            "{} declares no interactive session; run `stado stream declare {}` first",
            target.name, target.name
        ))
    })
}

fn record_stream_services(
    document: &mut Value,
    target: &crate::targets::ComputeTarget,
    declaration: &DisplayStream,
    managed_since: &str,
) -> Result<bool, CmdError> {
    let current = service::declared_services(target);
    let mut desired = remote::managed_services(target, declaration, managed_since);
    for wanted in &mut desired {
        if let Some(existing) = current
            .iter()
            .find(|existing| existing.matches(wanted.unit_id()))
        {
            wanted.host_heuristic = existing.host_heuristic.clone();
            wanted.onboarding = existing.onboarding.clone();
            if !existing.managed_since.is_empty() {
                wanted.managed_since = existing.managed_since.clone();
            }
        }
    }

    let existing_stream: Vec<_> = current
        .iter()
        .filter(|existing| {
            desired
                .iter()
                .any(|wanted| existing.matches(wanted.unit_id()))
        })
        .collect();
    if existing_stream.len() == desired.len()
        && existing_stream
            .iter()
            .zip(desired.iter())
            .all(|(existing, wanted)| *existing == wanted)
    {
        return Ok(false);
    }
    for existing in existing_stream {
        service::remove_service(document, &target.name, existing.unit_id()).map_err(click)?;
    }
    for wanted in desired {
        service::add_service(document, &wanted).map_err(click)?;
    }
    Ok(true)
}

pub(in crate::cli::stream) async fn apply(
    target_name: &str,
    provision_library: bool,
    json: bool,
) -> Result<(), CmdError> {
    // The unit files and their canonical service declarations are one
    // reconciliation. Hold the registry generation whose display declaration
    // drove the host install, then refuse a lost race instead of publishing
    // service definitions derived from an older stream declaration.
    let (mut document, expected_generation) = fetch_versioned_document().await?;
    let registry = crate::targets::load_registry_from_str(&serde_json::to_string(&document)?)
        .map_err(click)?;
    let target = registry
        .lookup(target_name)
        .ok_or_else(|| CmdError::click(format!("registry has no target named {target_name:?}")))?;
    let declaration = declaration_of(target)?;
    if !declaration.enabled {
        return Err(CmdError::click(format!(
            "{target_name} declares display_stream.enabled = false; nothing to apply"
        )));
    }
    let runner = production_runner();
    // The declaration names a board by driver UUID; Xorg addresses one by PCI
    // bus id, and only the host knows the mapping.
    let probed = remote::probe(target, &runner).await.map_err(click)?;
    let bus_id = remote::bus_id_for(&probed, declaration.gpu_uuid.as_deref()).ok_or_else(|| {
        CmdError::click(match &declaration.gpu_uuid {
            Some(uuid) => format!("{target_name} reports no board with uuid {uuid}"),
            None => format!("{target_name} reports no NVIDIA board at all"),
        })
    })?;
    let report = remote::install(target, &declaration, &bus_id, provision_library, &runner)
        .await
        .map_err(click)?;
    if report.get("status").and_then(Value::as_str) != Some("installed") {
        return Err(CmdError::click(format!(
            "stream operation did not reach installed: {report}"
        )));
    }
    let services_changed = record_stream_services(
        &mut document,
        target,
        &declaration,
        &chrono::Utc::now().to_rfc3339(),
    )?;
    let version = if services_changed {
        push_document_if(&document, &expected_generation).await?
    } else {
        expected_generation
    };
    let mut report = remote::with_declaration(report, &declaration);
    if let Some(map) = report.as_object_mut() {
        map.insert("store_version".to_string(), Value::String(version));
    }
    if !emitted(&report, json, "installed")? {
        return Ok(());
    }
    println!("{target_name}: session provisioned on {bus_id}");
    println!("  packages: {}", field(&report, "packages"));
    println!("  sunshine: {}", field(&report, "sunshine"));
    println!("  screen:   {}", field(&report, "session"));
    println!(
        "  units:    xorg {}, sunshine {}",
        field(&report, "xorg"),
        field(&report, "sunshine_state")
    );
    println!("  ports:    {}", field(&report, "ports"));
    println!("pair a client with `stado stream pair {target_name} --pin XXXX`");
    Ok(())
}
