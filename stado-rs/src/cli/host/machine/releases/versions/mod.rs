//! `stado host declare-version` and `stado host promote-version`.

pub(in crate::cli::host) mod promote;

use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::checks::probes::print_json;

/// `stado release declare-version --host TARGET --binary B --version V` says
/// what a host must run. `--unset` removes that declaration.
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
    version: Option<&str>,
    unset: bool,
    json: bool,
) -> Result<(), CmdError> {
    let binary = crate::deploy::products::product(binary)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let version = match (version, unset) {
        (Some(_), true) => {
            return Err(CmdError::usage(
                "--version and --unset cannot be used together",
            ));
        }
        (None, false) => {
            return Err(CmdError::usage(
                "one of --version or --unset must be provided",
            ));
        }
        (Some(version), false) => {
            let version = version.trim();
            if !crate::deploy::host_release::is_exact_semver(version) {
                return Err(CmdError::usage(
                    "--version must name an exact semantic version such as 0.5.1",
                ));
            }
            Some(version)
        }
        (None, true) => None,
    };
    let (mut document, expected_generation) =
        crate::cli::registry::fetch_versioned_document().await?;
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
        .ok_or_else(|| {
            CmdError::click(format!(
                "{target} is missing from registry.targets; add the host declaration before \
                 declaring a managed version"
            ))
        })?;

    if let Some(version) = version {
        let versions = entry
            .entry("managed_versions".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()))
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{target} declares managed_versions as a non-object; replace \
                     targets[].managed_versions with an object"
                ))
            })?;
        versions.insert(binary.name.to_string(), json!(version));
        let generation =
            crate::cli::registry::push_document_if(&document, &expected_generation).await?;
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
        return Ok(());
    }

    let removed = match entry.get_mut("managed_versions") {
        None => false,
        Some(versions) => versions
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{target} declares managed_versions as a non-object; replace \
                     targets[].managed_versions with an object"
                ))
            })?
            .remove(&binary.name)
            .is_some(),
    };
    let generation = if removed {
        crate::cli::registry::push_document_if(&document, &expected_generation).await?
    } else {
        expected_generation
    };
    if json {
        print_json(&json!({
            "target": target,
            "binary": binary.name,
            "removed": removed,
            "generation": generation,
        }));
    } else if removed {
        println!("{target}: {} declaration removed", binary.name);
    } else {
        println!("{target}: {} declaration is already absent", binary.name);
    }
    Ok(())
}
