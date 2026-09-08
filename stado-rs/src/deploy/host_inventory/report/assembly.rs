//! Report assembly: the observed inventory beside what the registry
//! declares, with one verdict field per comparison.

use serde_json::{json, Map, Value};

use super::super::*;
use crate::deploy::host_channel;
use crate::targets::{ComputeTarget, ServiceDirectory};

/// The inventory as the `--json` report, in `host reboot`'s report shape.
///
/// `directory` is the registry's service directory, when the document
/// carries one. Together with `target.managed_versions` it is the DECLARED
/// state; `inventory` is the observed state; every `*_verdict` field below
/// is one comparison of the two.
pub fn to_report(
    target: &ComputeTarget,
    directory: Option<&ServiceDirectory>,
    inventory: &Inventory,
) -> Map<String, Value> {
    // Axis one: the version each managed binary runs against the version
    // the registry requires of this host.
    let mut binaries = Vec::with_capacity(inventory.managed_binaries.len());
    let mut versions_behind: Vec<&str> = Vec::new();
    let mut versions_ahead: Vec<&str> = Vec::new();
    let mut versions_mismatched: Vec<&str> = Vec::new();
    let mut versions_unjudged: Vec<&str> = Vec::new();
    let mut versions_undeclared: Vec<&str> = Vec::new();
    for binary in &inventory.managed_binaries {
        let declared = target.declared_version(&binary.name);
        let state = version_verdict(binary, declared);
        match state {
            BEHIND => versions_behind.push(&binary.name),
            AHEAD => versions_ahead.push(&binary.name),
            MISMATCHED => versions_mismatched.push(&binary.name),
            UNDECLARED => versions_undeclared.push(&binary.name),
            MATCHED => {}
            _ => versions_unjudged.push(&binary.name),
        }
        binaries.push(json!({
            "name": binary.name,
            "state": binary.state,
            "regular_file": binary.regular_file,
            "executable": binary.executable,
            "version_state": binary.version_state,
            "version": binary.version,
            "declared_version": declared,
            "version_verdict": state,
        }));
    }

    // Axes two and three, per marker and independent of each other: does
    // anything answer where the marker points, and does the marker point
    // where the registry declares this host answers.
    let mut markers = Vec::with_capacity(inventory.forwards.len());
    let mut stale_markers: Vec<&str> = Vec::new();
    let mut disagreeing_markers: Vec<&str> = Vec::new();
    let mut undeclared_markers: Vec<&str> = Vec::new();
    let mut matched = usize::MIN;
    let mut stale = usize::MIN;
    let mut unreadable = usize::MIN;
    let mut unknown = usize::MIN;
    let mut declared_matched = usize::MIN;
    for marker in &inventory.forwards {
        let (port, state) = verdict(marker, &inventory.listeners, &inventory.listeners_state);
        match state {
            MATCHED => matched += 1,
            STALE => {
                stale += 1;
                stale_markers.push(&marker.name);
            }
            UNKNOWN => unknown += 1,
            _ => unreadable += 1,
        }
        // Two declared sources, in the order `service directory publish`
        // writes them: the endpoint this host serves on, then the adapter it
        // dials when the service lives elsewhere. A marker is judged against
        // whichever the host is entitled to, so a correct adapter address
        // stops reading as `undeclared`.
        let served = declared_endpoint(directory, target, &marker.name);
        let adapter = if served.is_some() {
            None
        } else {
            declared_adapter(target, &marker.name)
        };
        let declared_url = served.map(str::to_string).or_else(|| adapter.clone());
        let declaration = declaration_verdict(marker, declared_url.as_deref());
        match declaration {
            DISAGREES => disagreeing_markers.push(&marker.name),
            UNDECLARED => undeclared_markers.push(&marker.name),
            _ => declared_matched += 1,
        }
        markers.push(json!({
            "name": marker.name,
            "state": marker.state,
            "url": marker.url,
            "port": port,
            "reconciliation": state,
            "declared_url": declared_url,
            "declared_source": if served.is_some() {
                "directory-endpoint"
            } else if adapter.is_some() {
                "resolver-adapter"
            } else {
                "undeclared"
            },
            "declaration_verdict": declaration,
        }));
    }

    // The two vault findings, not the raw table above them. A vault whose
    // group or other bits are set, and a vault path that was refused, are
    // both conclusions an operator should never have to derive by reading
    // a mode column. Only regular files can be judged owner-only: a symlink
    // is lrwxrwxrwx by construction, so listing one here would report the
    // link's permissions as a vault's and drown the real finding.
    let mut vaults_not_owner_only: Vec<&str> = Vec::new();
    let mut vaults_refused: Vec<&str> = Vec::new();
    for vault in &inventory.vaults {
        if vault.state != VAULT_REGULAR {
            vaults_refused.push(&vault.name);
        } else if !vault.owner_only {
            vaults_not_owner_only.push(&vault.name);
        }
    }

    let mut report = host_channel::base_report(target);
    report.insert(
        "sanitizer_state".to_string(),
        json!(inventory.sanitizer_state),
    );
    report.insert(
        "release_platform".to_string(),
        json!(inventory.release_platform),
    );
    report.insert(
        "declared_release_platform".to_string(),
        json!(target.release_platform),
    );
    report.insert(
        "release_platform_verdict".to_string(),
        json!(if inventory.release_platform == target.release_platform {
            MATCHED
        } else {
            MISMATCHED
        }),
    );
    report.insert(
        "forwards_dir_state".to_string(),
        json!(inventory.forwards_dir_state),
    );
    report.insert("managed_binaries".to_string(), json!(binaries));
    // Reported as the host answered them, with both epochs, because the
    // comparison an operator needs — is this unit executing something older
    // than what the fleet installed — is not derivable from either number
    // alone.
    report.insert(
        "service_artifacts".to_string(),
        json!(inventory.service_artifacts),
    );
    report.insert("cargo".to_string(), json!(inventory.cargo));
    report.insert("forwards".to_string(), json!(markers));
    report.insert("listeners".to_string(), json!(inventory.listeners));
    report.insert(
        "listeners_state".to_string(),
        json!(inventory.listeners_state),
    );
    report.insert("subcommands".to_string(), json!(inventory.subcommands));
    report.insert("vaults".to_string(), json!(inventory.vaults));
    report.insert("vaults_seen".to_string(), json!(inventory.vaults_seen));
    report.insert(
        "vaults_truncated".to_string(),
        json!(inventory.vaults_seen > inventory.vaults.len() as u64),
    );
    report.insert(
        "vault_sidecars".to_string(),
        json!(inventory.vault_sidecars),
    );
    report.insert(
        "vault_sidecars_seen".to_string(),
        json!(inventory.vault_sidecars_seen),
    );
    report.insert(
        "vault_sidecars_truncated".to_string(),
        json!(inventory.vault_sidecars_seen > inventory.vault_sidecars.len() as u64),
    );
    report.insert(
        "reconciliation".to_string(),
        json!({
            "markers": inventory.forwards.len(),
            "matched": matched,
            "stale": stale,
            "unreadable": unreadable,
            "unknown": unknown,
            "stale_markers": stale_markers,
            // The registry axis, counted separately from the listener axis
            // above it on purpose: they are different questions, and one
            // combined "drift" number would hide which of them was answered.
            "declaration_matched": declared_matched,
            "declaration_disagrees": disagreeing_markers.len(),
            "declaration_undeclared": undeclared_markers.len(),
            "disagreeing_markers": disagreeing_markers,
            "undeclared_markers": undeclared_markers,
            "versions_behind": versions_behind,
            "versions_ahead": versions_ahead,
            "versions_mismatched": versions_mismatched,
            "versions_unjudged": versions_unjudged,
            "versions_undeclared": versions_undeclared,
            "vaults_not_owner_only": vaults_not_owner_only,
            "vaults_refused": vaults_refused,
        }),
    );
    report
}
