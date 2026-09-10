//! What a signed release manifest must say before it is accepted.

use std::collections::BTreeSet;

use crate::binary::release::canonical_coordinate;
use crate::release_control::{QualificationStatus, ReleaseManifest, MAX_RELEASE_BYTES};

use super::shape::{identifier, safe_relative, sha256};

pub fn validate_manifest(manifest: &ReleaseManifest) -> Result<(), String> {
    if manifest.schema_version != 1 {
        return Err("release manifest schema_version must be 1".to_string());
    }
    for (name, value) in [
        ("product", manifest.product.as_str()),
        ("version", manifest.version.as_str()),
        ("platform", manifest.platform.as_str()),
        ("key_id", manifest.key_id.as_str()),
    ] {
        if !identifier(value) {
            return Err(format!(
                "release manifest {name} is not a canonical coordinate"
            ));
        }
    }
    if manifest.source_revision.len() != 40
        || !manifest
            .source_revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(
            "release manifest source_revision must be a full lowercase Git commit".to_string(),
        );
    }
    for (name, value) in [
        ("artifact_sha256", manifest.artifact_sha256.as_str()),
        ("source_sha256", manifest.source_sha256.as_str()),
        (
            "pipeline_manifest_sha256",
            manifest.pipeline_manifest_sha256.as_str(),
        ),
        (
            "qualification_receipt_sha256",
            manifest.qualification_receipt_sha256.as_str(),
        ),
    ] {
        if !sha256(value) {
            return Err(format!(
                "release manifest {name} must be 64 lowercase hexadecimal characters"
            ));
        }
    }
    if manifest.artifact_bytes == 0 || manifest.artifact_bytes > MAX_RELEASE_BYTES {
        return Err("release manifest artifact_bytes is outside the supported range".to_string());
    }
    let runtime_present = !manifest.binary.is_empty()
        || !manifest.launcher.is_empty()
        || manifest.config_schema != 0
        || manifest.state_schema != 0
        || !manifest.minimum_stado_version.is_empty()
        || !manifest.rollback_compatible_with.is_empty();
    if runtime_present
        && (!safe_relative(&manifest.binary)
            || !safe_relative(&manifest.launcher)
            || manifest.config_schema == 0
            || manifest.state_schema == 0
            || !canonical_coordinate(&manifest.minimum_stado_version))
    {
        return Err("release manifest runtime fields are incomplete or invalid".to_string());
    }
    let mut rollback = BTreeSet::new();
    for version in &manifest.rollback_compatible_with {
        if !canonical_coordinate(version) || !rollback.insert(version) {
            return Err(
                "release manifest rollback_compatible_with is invalid or duplicated".to_string(),
            );
        }
    }
    match manifest.qualification.status {
        QualificationStatus::Passed => {
            if !manifest
                .qualification
                .evidence_sha256
                .as_deref()
                .is_some_and(sha256)
                || manifest
                    .qualification
                    .completed_at
                    .as_deref()
                    .unwrap_or("")
                    .is_empty()
            {
                return Err(
                    "passed release qualification requires evidence_sha256 and completed_at"
                        .to_string(),
                );
            }
        }
        QualificationStatus::Pending | QualificationStatus::Failed => {}
    }
    Ok(())
}
