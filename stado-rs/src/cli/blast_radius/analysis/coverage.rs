//! Backup namespace coverage: whether the configured DR endpoint is an
//! independent failure domain, and whether it holds the primary's names.

use crate::cli::blast_radius::{CoverageReport, StorageInspection};
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
    if backup.report.state != "reachable" {
        return CoverageReport {
            state: "backup_unreadable".to_string(),
            missing_from_backup: None,
            extra_only_in_backup: None,
            explanation: "the backup endpoint is configured but could not be listed".to_string(),
        };
    }
    if primary.report.state != "reachable" {
        return CoverageReport {
            state: "backup_readable_primary_unknown".to_string(),
            missing_from_backup: None,
            extra_only_in_backup: None,
            explanation: "backup objects are readable, but primary failure prevents an RPO or completeness comparison".to_string(),
        };
    }

    let mut missing = usize::default();
    let mut extra = usize::default();
    for prefix in CANONICAL_PREFIXES {
        let primary_names = primary.names.get(*prefix).cloned().unwrap_or_default();
        let backup_names = backup.names.get(*prefix).cloned().unwrap_or_default();
        missing = missing.saturating_add(primary_names.difference(&backup_names).count());
        extra = extra.saturating_add(backup_names.difference(&primary_names).count());
    }
    let state = if missing == usize::default() {
        "namespace_covered"
    } else {
        "incomplete"
    };
    CoverageReport {
        state: state.to_string(),
        missing_from_backup: Some(missing),
        extra_only_in_backup: Some(extra),
        explanation: "coverage compares canonical object names; content and metadata integrity remain the responsibility of storage copy verify".to_string(),
    }
}
