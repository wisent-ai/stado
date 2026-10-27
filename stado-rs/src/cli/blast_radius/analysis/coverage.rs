//! Backup namespace coverage: whether the configured DR endpoint is an
//! independent failure domain, and whether it holds the primary's names.

use crate::cli::blast_radius::{CoverageReport, Reach, StorageInspection};
use crate::queue::copy::CANONICAL_PREFIXES;

pub(in crate::cli::blast_radius) fn compare_coverage(
    primary: &StorageInspection,
    backup: &StorageInspection,
    configured: bool,
    matches_primary: bool,
) -> CoverageReport {
    if !configured {
        return CoverageReport {
            state: "not_configured".to_string(),
            missing_from_backup: None,
            extra_only_in_backup: None,
            explanation: "no disaster-recovery endpoint is configured; queue state has no Stado-managed backup".to_string(),
        };
    }
    if matches_primary {
        return CoverageReport {
            state: "same_as_primary_not_a_backup".to_string(),
            missing_from_backup: None,
            extra_only_in_backup: None,
            explanation: "the backup locator resolves to the primary store and provides no independent failure domain".to_string(),
        };
    }
    if backup.report.state != Reach::Reachable {
        return CoverageReport {
            state: "backup_unreadable".to_string(),
            missing_from_backup: None,
            extra_only_in_backup: None,
            explanation: "the backup endpoint is configured but could not be listed".to_string(),
        };
    }
    if primary.report.state != Reach::Reachable {
        return CoverageReport {
            state: "backup_readable_primary_unknown".to_string(),
            missing_from_backup: None,
            extra_only_in_backup: None,
            explanation: "backup objects are readable, but primary failure prevents an RPO or completeness comparison".to_string(),
        };
    }

    // A reachable listing holds every canonical prefix; one that lacks a
    // prefix was not listed whole, so no completeness is claimed from it.
    let mut missing = Vec::new();
    let mut extra = Vec::new();
    for prefix in CANONICAL_PREFIXES {
        let (Some(primary_names), Some(backup_names)) =
            (primary.names.get(*prefix), backup.names.get(*prefix))
        else {
            return CoverageReport {
                state: "listing_incomplete".to_string(),
                missing_from_backup: None,
                extra_only_in_backup: None,
                explanation: format!(
                    "the {prefix} prefix was not listed on both stores, so coverage cannot be compared"
                ),
            };
        };
        missing.extend(primary_names.difference(backup_names));
        extra.extend(backup_names.difference(primary_names));
    }
    let state = if missing.is_empty() {
        "namespace_covered"
    } else {
        "incomplete"
    };
    CoverageReport {
        state: state.to_string(),
        missing_from_backup: Some(missing.len()),
        extra_only_in_backup: Some(extra.len()),
        explanation: "coverage compares canonical object names; content and metadata integrity remain the responsibility of storage copy verify".to_string(),
    }
}
