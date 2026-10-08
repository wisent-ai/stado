use crate::targets::*;

/// Validate a candidate document against the one it would replace, scoping an
/// `inference` failure to writes that actually touch `inference`.
///
/// The whole document used to be refused for any failure anywhere, and that
/// blast radius was the defect: a single field — an `inference` route set
/// to `"best"` while `inference.deployments` is empty — freezes every write
/// in every domain:
/// `release version declare|promote`, `service adopt`, a `disk_cleanup`
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
    let mut kept = Vec::new();
    // The two sections a write is held to only when it changes them: model
    // routes (above), and the unit-image revisit policy, whose labels a
    // catalog rename can invalidate without anyone touching the block
    // (000d82b6). A fault already in an unchanged section is returned for
    // the caller to report, never swallowed.
    let scoped = [
        (
            "inference",
            crate::inference::schema::validate(candidate).err(),
        ),
        (
            crate::release_unit_image::REVISIT_POLICY_KEY,
            crate::release_unit_image::validate_registry_contract(candidate).err(),
        ),
    ];
    for (section, failure) in scoped {
        let Some(failure) = failure else { continue };
        let unchanged =
            current.is_some_and(|current| candidate.get(section) == current.get(section));
        if !unchanged {
            return Err(RegistryValidationError(failure));
        }
        kept.push(format!("`{section}`: {failure}"));
    }
    Ok((!kept.is_empty()).then(|| kept.join("; ")))
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
