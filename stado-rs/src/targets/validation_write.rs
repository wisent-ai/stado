use super::*;

/// Validate a candidate document against the one it would replace, scoping an
/// `inference` failure to writes that actually touch `inference`.
///
/// The whole document used to be refused for any failure anywhere, and that
/// blast radius was the defect. On 2026-08-31 a single field —
/// `inference.routes["wisent-backend/evaluation"]` set to `"best"` while
/// `inference.deployments` was empty — froze every write in every domain:
/// `declare-version`, `promote-version`, `service adopt`, a `disk_cleanup`
/// edit, all of it. A release could not be declared for a host because of a
/// model route it never touches.
///
/// So: everything outside `inference` must always validate. An `inference`
/// failure refuses the write only when the write CHANGES `inference`. A
/// candidate whose `inference` section is byte-identical to the current one
/// cannot have introduced the fault and cannot make it worse, so it proceeds
/// and the pre-existing fault is returned for the caller to report rather than
/// swallowed.
///
/// This narrows the gate; it does not remove it. A write that edits `inference`
/// is held to the full check exactly as before.
pub fn validate_registry_for_write(
    candidate: &Value,
    current: Option<&Value>,
) -> Result<Option<String>, RegistryValidationError> {
    validate_registry_body(candidate, false)?;
    let Err(inference_error) = crate::inference::schema::validate(candidate) else {
        return Ok(None);
    };
    let unchanged =
        current.is_some_and(|current| candidate.get("inference") == current.get("inference"));
    if unchanged {
        Ok(Some(inference_error))
    } else {
        Err(RegistryValidationError(inference_error))
    }
}

/// Load and validate a registry-v2 JSON file.
pub fn validate_registry_file(path: &Path) -> Result<Value, RegistryValidationError> {
    let text = std::fs::read_to_string(path)
        .map_err(|exc| RegistryValidationError(format!("{}: {exc}", path.display())))?;
    let data: Value = serde_json::from_str(&text)
        .map_err(|exc| RegistryValidationError(format!("{}: {exc}", path.display())))?;
    validate_registry(&data)?;
    Ok(data)
}
