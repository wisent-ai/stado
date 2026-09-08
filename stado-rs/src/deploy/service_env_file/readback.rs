//! The write path seen from the other end: read the file back over the shared
//! channel, say whether the one key a writer asked about still holds what it
//! wrote, name whatever overwrote it, and render the whole thing as the
//! `--json` report.

use serde_json::{json, Map, Value};

use super::*;
use crate::deploy::{host_channel, host_inventory, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The read-back verdict for the one key a write asked about.
///
/// The report's own word, except when the report says the file was never
/// opened or its assignments were never parsed. A writer that treated an
/// unreadable file as "not overwritten" would report success for a write
/// nobody checked, which is the failure this whole path exists to remove.
pub fn expectation(report: &EnvFileReport) -> &str {
    if report.file_state != FILE_READ || report.entries_state != ENTRIES_READ {
        return EXPECT_UNVERIFIED;
    }
    &report.expected
}

/// The assignment of `key` a shell that sourced this file would end up with:
/// the LAST one, in file order.
pub fn effective_entry<'a>(report: &'a EnvFileReport, key: &str) -> Option<&'a EnvEntry> {
    report
        .entries
        .iter()
        .rfind(|entry| entry.form != FORM_UNPARSABLE && entry.key == key)
}

/// The forward marker whose URL is exactly `value`, when one holds it.
///
/// This is the attribution that turns "your write was overwritten" into an
/// actionable sentence. A host-side reconciler that rewrites an env key from a
/// marker leaves the marker's own text in the file, so a marker holding
/// exactly what came back names the declaration the operator must correct
/// instead of the file they will otherwise keep fighting. Exact match only: a
/// marker that merely shares a port is a guess, and a wrong attribution sends
/// an operator to the wrong file.
pub fn marker_holding<'a>(
    markers: &'a [host_inventory::ForwardMarker],
    value: &str,
) -> Option<&'a host_inventory::ForwardMarker> {
    let value = value.trim();
    markers
        .iter()
        .find(|marker| marker.state == host_inventory::MARKER_READ && marker.url.trim() == value)
}

/// Read this host's forward markers, for [`marker_holding`].
///
/// Deliberately a second round trip, taken only when a write did not survive:
/// the markers are not needed to answer "did it survive", and a reader that
/// collected them every time would make the common path pay for the rare
/// diagnosis. The inventory script is reused rather than reimplemented so this
/// command and `host inventory` cannot disagree about what a marker says.
pub async fn forward_markers(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Vec<host_inventory::ForwardMarker>, DeployError> {
    let script = host_inventory::remote_inventory_script()?;
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(host_inventory::parse_inventory(&output.stdout)?.forwards)
}

/// The env file as the `--json` report, in `host inventory`'s report shape.
pub fn to_report(target: &ComputeTarget, unit: &str, report: &EnvFileReport) -> Map<String, Value> {
    let mut payload = host_channel::base_report(target);
    payload.insert("unit".to_string(), json!(unit));
    payload.insert("env_file".to_string(), json!(report.path));
    payload.insert("file_state".to_string(), json!(report.file_state));
    payload.insert("detail".to_string(), json!(report.detail));
    payload.insert("mode".to_string(), json!(report.mode));
    payload.insert("owner_only".to_string(), json!(report.owner_only));
    payload.insert("bytes".to_string(), json!(report.bytes));
    payload.insert("entries_state".to_string(), json!(report.entries_state));
    payload.insert("entries_seen".to_string(), json!(report.entries_seen));
    let roles = shadowing(&report.entries);
    payload.insert(
        "entries".to_string(),
        Value::Array(
            report
                .entries
                .iter()
                .zip(&roles)
                .map(|(entry, role)| {
                    json!({
                        "line": entry.line,
                        "form": entry.form,
                        "key": entry.key,
                        "value_state": entry.value_state,
                        "value": entry.value,
                        "chars": entry.chars,
                        "resolution": role,
                    })
                })
                .collect(),
        ),
    );
    payload.insert(
        "duplicate_keys".to_string(),
        json!(duplicate_keys(&report.entries)),
    );
    payload.insert(
        "redacted".to_string(),
        json!(report
            .entries
            .iter()
            .filter(|entry| entry.value_state == VALUE_REDACTED)
            .count()),
    );
    payload.insert("expected".to_string(), json!(expectation(report)));
    payload
}

/// Read one already-resolved registry host's env file.
///
/// Split out from the CLI for the reason
/// [`super::host_inventory::inventory_target`](crate::deploy::host_inventory::inventory_target) is: the whole read is
/// exercisable through the [`Runner`] seam without a registry.
pub async fn read_env_file(
    target: &ComputeTarget,
    request: &EnvFileRequest<'_>,
    runner: &Runner,
) -> Result<EnvFileReport, DeployError> {
    let script = remote_env_file_script(request);
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {}",
            target.name,
            host_channel::last_error_line(&output, "ssh failed")
        )));
    }
    parse_env_file(&output.stdout)
}
