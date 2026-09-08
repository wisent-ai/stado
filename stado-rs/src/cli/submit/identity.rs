//! The identities a submission pins at submit time: the artifact refs it
//! resolves to immutable versions and their manifest digests, the scoped
//! secret references it names, and the consumer a hard-pinned job is bound
//! to. Each one is resolved before the request is assembled, so the digests
//! the receipt reports describe exactly what was submitted.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::cli::CmdError;

/// Python `_resolve_input_artifacts`: validates the NAME=REF shape and
/// name safety, then resolves each ref through the artifacts registry at
/// submit time (aliases resolve to their immutable version) into
/// `resolved_input_artifacts` entries of `{"ref", "uri",
/// "manifest_sha256"}` — exactly the maps `cli.py` threads into the job.
pub(super) async fn resolve_input_artifacts(
    values: &[String],
) -> Result<(Map<String, Value>, Map<String, Value>), CmdError> {
    let name_re =
        regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_-]{0,63}$").expect("static regex compiles");
    // The registry is built lazily: with no --input-artifact flags there is
    // nothing to resolve (Python constructs JobStorage eagerly, but its
    // constructor performs no I/O either).
    let registry = if values.is_empty() {
        None
    } else {
        Some(
            crate::artifacts::ArtifactRegistry::new()
                .await
                .map_err(|exc| CmdError::click(exc.to_string()))?,
        )
    };
    let mut requested = Map::new();
    let mut resolved = Map::new();
    for value in values {
        let Some((name, reference)) = value.split_once('=') else {
            return Err(CmdError::click(format!(
                "--input-artifact must be NAME=REF: '{value}'"
            )));
        };
        if !name_re.is_match(name) {
            return Err(CmdError::click(format!(
                "artifact input name is unsafe: '{name}'"
            )));
        }
        if requested.contains_key(name) {
            return Err(CmdError::click(format!(
                "duplicate artifact input name: {name}"
            )));
        }
        let manifest = registry
            .as_ref()
            .expect("registry exists when values are non-empty")
            .resolve_manifest(&crate::artifacts_models::ArtifactRef::parse(reference)?)
            .await?;
        let primary = manifest
            .locations
            .iter()
            .find(|location| location.role == "primary")
            .ok_or_else(|| {
                CmdError::click(format!(
                    "artifact has no primary location: {}",
                    manifest.ref_
                ))
            })?;
        requested.insert(name.to_string(), Value::from(reference));
        let mut resolved_input = Map::from_iter([
            ("ref".into(), Value::from(manifest.ref_.to_string())),
            ("uri".into(), Value::from(primary.uri.clone())),
            (
                "manifest_sha256".into(),
                Value::from(manifest.verification.manifest_sha256.clone()),
            ),
        ]);
        if primary.uri.starts_with("stado://") {
            resolved_input.insert("stado_uri".into(), Value::from(primary.uri.clone()));
            resolved_input.insert(
                "relative_path".into(),
                Value::from(format!("inputs/{name}")),
            );
            if !primary.sha256.is_empty() {
                resolved_input.insert("sha256".into(), Value::from(primary.sha256.clone()));
            }
        }
        resolved.insert(name.to_string(), Value::Object(resolved_input));
    }
    Ok((requested, resolved))
}
pub(crate) fn parse_secret_env(
    values: &[String],
) -> Result<BTreeMap<String, crate::models::JobSecretRef>, CmdError> {
    let env_re = regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("static regex compiles");
    let item_re =
        regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._:-]*$").expect("static regex compiles");
    let field_re =
        regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*$").expect("static regex compiles");
    let mut parsed = BTreeMap::new();
    for value in values {
        let Some((env_name, reference)) = value.split_once('=') else {
            return Err(CmdError::click(format!(
                "--secret-env must be ENV_NAME=SKARBIEC_ITEM#FIELD: {value:?}"
            )));
        };
        let Some((item, field)) = reference.split_once('#') else {
            return Err(CmdError::click(format!(
                "--secret-env must be ENV_NAME=SKARBIEC_ITEM#FIELD: {value:?}"
            )));
        };
        if !env_re.is_match(env_name) || !item_re.is_match(item) || !field_re.is_match(field) {
            return Err(CmdError::click(format!(
                "--secret-env contains an unsafe environment, item, or field name: {value:?}"
            )));
        }
        if !crate::config::agent_secret_reference_allowed(item, field) {
            return Err(CmdError::click(format!(
                "--secret-env reference {item}#{field} is not in agent.skarbiec.secret_fields"
            )));
        }
        if parsed
            .insert(
                env_name.to_string(),
                crate::models::JobSecretRef {
                    item: item.to_string(),
                    field: field.to_string(),
                },
            )
            .is_some()
        {
            return Err(CmdError::click(format!(
                "duplicate --secret-env variable: {env_name}"
            )));
        }
    }
    Ok(parsed)
}

/// Turn an operator-facing registry target into the consumer id stored on a
/// hard-pinned job. The configured registry is authoritative: the bundled
/// snapshot may predate the target the running worker was started from.
pub(in crate::cli) async fn resolve_pinned_host(value: &str) -> Result<String, CmdError> {
    if value.is_empty() {
        return Ok(String::new());
    }
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let Some(target) = registry.lookup(value) else {
        return Ok(value.to_string());
    };
    let Some(hostname) = target.hostnames.first() else {
        return Err(CmdError::click(format!(
            "--pinned-host target '{value}' has no hostnames[] in the registry; \
             cannot derive its consumer_id."
        )));
    };
    Ok(format!("{}-{hostname}", target.kind))
}
