//! What a signed release manifest must say before it is accepted.

use std::collections::BTreeSet;
use std::num::NonZeroU64;

use crate::binary::provenance::is_commit_id;
use crate::binary::release::canonical_coordinate;
use crate::release_control::{
    QualificationStatus, ReleaseManifest, RELEASE_MANIFEST_SCHEMA_VERSION,
};

use super::shape::{identifier, safe_relative, sha256};

/// Whether a count the manifest states is unset: zero is no count.
fn unset(count: u64) -> bool {
    NonZeroU64::new(count).is_none()
}

pub fn validate_manifest(manifest: &ReleaseManifest) -> Result<(), String> {
    if manifest.schema_version != RELEASE_MANIFEST_SCHEMA_VERSION {
        return Err(format!(
            "release manifest schema_version must be {RELEASE_MANIFEST_SCHEMA_VERSION}"
        ));
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
    if !is_commit_id(&manifest.source_revision)
        || manifest
            .source_revision
            .bytes()
            .any(|byte| byte.is_ascii_uppercase())
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
    if unset(manifest.artifact_bytes) {
        return Err("release manifest artifact_bytes must be positive".to_string());
    }
    let runtime_present = !manifest.binary.is_empty()
        || !manifest.launcher.is_empty()
        || !unset(manifest.config_schema)
        || !unset(manifest.state_schema)
        || !manifest.minimum_stado_version.is_empty()
        || !manifest.rollback_compatible_with.is_empty();
    if runtime_present
        && (!safe_relative(&manifest.binary)
            || !safe_relative(&manifest.launcher)
            || unset(manifest.config_schema)
            || unset(manifest.state_schema)
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
            let qualification = &manifest.qualification;
            if !qualification.evidence_sha256.as_deref().is_some_and(sha256) {
                return Err(
                    "passed release qualification requires evidence_sha256, a SHA-256 digest"
                        .to_string(),
                );
            }
            if !qualification
                .completed_at
                .as_deref()
                .is_some_and(|at| !at.trim().is_empty())
            {
                return Err("passed release qualification requires completed_at".to_string());
            }
        }
        QualificationStatus::Pending | QualificationStatus::Failed => {}
    }
    Ok(())
}
