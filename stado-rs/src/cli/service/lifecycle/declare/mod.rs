//! `service declare` and `service ensure`: the two commands that write what
//! a host must run — one from a declaration an operator authored, one from
//! the facts the host itself reports.

use super::*;

pub(crate) mod ensure;

/// One declared service name: lowercase letters and digits at the edges,
/// with '.', '-' and '_' inside — the same rule the directory validator
/// applies, enforced here so a bad name fails before the document moves.
fn declaration_name_ok(value: &str) -> bool {
    let edge = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && edge(bytes[0])
        && edge(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|byte| edge(*byte) || matches!(byte, b'.' | b'_' | b'-'))
}

/// Write one user-authored declaration into the service directory.
///
/// The declaration file is the whole contract: `name` and `host`, the
/// immutable `source`, the opaque `run` spec, and optionally `verify`,
/// `consumers`, and `endpoints` (or `port` as the loopback shorthand).
/// The fleet learns nothing about the service's kind from it — that is the
/// design, not a gap: anything the service knows about itself lives inside
/// the artifact and the run spec.
pub(crate) async fn declare(file: &str, as_json: bool) -> Result<(), CmdError> {
    let text = std::fs::read_to_string(file)
        .map_err(|error| CmdError::click(format!("{file}: {error}")))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| CmdError::click(format!("{file}: not a JSON object: {error}")))?;
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::usage(format!("{file}: 'name' is required")))?;
    if !declaration_name_ok(name) {
        return Err(CmdError::usage(format!(
            "{file}: 'name' must be a lowercase identifier without empty edges"
        )));
    }
    let host = value
        .get("host")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::usage(format!("{file}: 'host' is required")))?;
    let mut declaration_value = match value.get("declaration") {
        Some(declaration) => declaration.clone(),
        None => {
            let mut assembled = json!({"source": value.get("source")});
            if let Some(run) = value.get("run") {
                assembled["run"] = run.clone();
            }
            assembled
        }
    };
    let declaration: crate::declaration::ServiceDeclaration =
        serde_json::from_value(std::mem::take(&mut declaration_value)).map_err(|error| {
            CmdError::usage(format!(
                "{file}: declaration must carry source.artifact and source.sha256: {error}"
            ))
        })?;
    let location = format!("service_directory.services.{name}");
    let problems = crate::declaration::validate(&location, &declaration);
    if !problems.is_empty() {
        return Err(CmdError::usage(problems.join("; ")));
    }
    let verify = match value.get("verify") {
        Some(descriptor) => {
            let descriptor: targets::VerifyDescriptor = serde_json::from_value(descriptor.clone())
                .map_err(|error| CmdError::usage(format!("{file}: 'verify': {error}")))?;
            let problems = targets::validate_verification(&location, &descriptor);
            if !problems.is_empty() {
                return Err(CmdError::usage(problems.join("; ")));
            }
            Some(
                serde_json::to_value(&descriptor)
                    .map_err(|error| CmdError::click(format!("verify: {error}")))?,
            )
        }
        None => None,
    };
    // Endpoints: explicit map wins; `port` is the loopback shorthand for the
    // declared host. The directory contract requires at least the active
    // host's endpoint, so neither present is an author error, not a default.
    let mut endpoints = value
        .get("endpoints")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let (Some(port), false) = (
        value.get("port").and_then(Value::as_u64),
        endpoints.contains_key(host),
    ) {
        if port == 0 || port > u16::MAX as u64 {
            return Err(CmdError::usage(format!("{file}: 'port' out of range")));
        }
        endpoints.insert(
            host.to_string(),
            json!({"url": format!("http://127.0.0.1:{port}")}),
        );
    }
    if !endpoints.contains_key(host) {
        return Err(CmdError::usage(format!(
            "{file}: the declaration needs an endpoint for {host} — pass 'endpoints' or the 'port' shorthand"
        )));
    }
    // The directory answers "who may call it" from the same entry, so a
    // declaration without consumers is not a declaration the contract can
    // publish: it would answer that question with silence.
    match value.get("consumers").and_then(Value::as_object) {
        Some(consumers) if !consumers.is_empty() => {}
        _ => {
            return Err(CmdError::usage(format!(
                "{file}: 'consumers' is required and must name at least one caller"
            )));
        }
    }

    // Pure: the whole entry is a function of the file that was just parsed
    // and of the document it lands in, so a lost race re-applies it to the
    // newer document. `advance_generation` runs INSIDE the transform because
    // it derives the next counter from the document it was handed; deriving
    // it once from the first read and reusing it after a retry would publish
    // a number the newer document has already used.
    let generation = registry::commit_document(|current| {
        let mut document = current.clone();
        let known_target = document
            .get("targets")
            .and_then(Value::as_array)
            .is_some_and(|targets| {
                targets
                    .iter()
                    .any(|target| target.get("name").and_then(Value::as_str) == Some(host))
            });
        if !known_target {
            return Err(CmdError::usage(format!(
                "{file}: 'host' names {host}, which is not a registry target"
            )));
        }
        let directory = document
            .get_mut("service_directory")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(
                    "registry has no service_directory; an authority must publish it before services can be declared",
                )
            })?;
        let services = directory
            .entry("services")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| CmdError::click("service_directory.services: must be an object"))?;
        let entry = services
            .entry(name.to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::click(format!(
                    "service_directory.services.{name}: must be an object"
                ))
            })?;
        entry.insert("active_host".to_string(), json!(host));
        entry.insert("endpoints".to_string(), Value::Object(endpoints.clone()));
        // The directory contract binds every fixed route to the managed unit on
        // its active host, and the unit lands when `deploy` runs — so declare
        // writes the link now and a placeholder record on the host, which
        // `deploy` replaces with the real one. Declared-but-not-yet-deployed is
        // a designed state, not an error: the registry says what should run,
        // the beacons say what does.
        entry.insert("managed_service".to_string(), json!(name));
        if let Some(consumers) = value.get("consumers") {
            entry.insert("consumers".to_string(), consumers.clone());
        }
        if let Some(descriptor) = verify.clone() {
            entry.insert("verify".to_string(), descriptor);
        }
        entry.insert(
            "declaration".to_string(),
            serde_json::to_value(&declaration)
                .map_err(|error| CmdError::click(format!("declaration: {error}")))?,
        );
        let target_entry = document
            .get_mut("targets")
            .and_then(Value::as_array_mut)
            .and_then(|targets| {
                targets
                    .iter_mut()
                    .find(|target| target.get("name").and_then(Value::as_str) == Some(host))
            })
            .ok_or_else(|| CmdError::click(format!("registry targets lost {host}")))?;
        let host_services = target_entry
            .as_object_mut()
            .ok_or_else(|| CmdError::click("registry target: must be an object"))?
            .entry("services")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| {
                CmdError::click(format!("registry target {host}: services must be an array"))
            })?;
        let already = host_services
            .iter()
            .any(|record| record.get("name").and_then(Value::as_str) == Some(name));
        if !already {
            host_services.push(json!({"name": name, "declared_only": true}));
        }
        // `declare` writes `service_directory.services.<name>` above, so the
        // publication counter must advance with it or a consumer's cached copy
        // never learns the entry exists.
        crate::service_resolution::advance_generation(&mut document).map_err(CmdError::click)?;
        Ok(document)
    })
    .await?;
    if !as_json {
        println!("declared {name} on {host} (generation {generation})");
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "declared": name,
                "host": host,
                "generation": generation,
            }))
            .map_err(|error| CmdError::click(format!("declare report: {error}")))?
        );
    }
    Ok(())
}
