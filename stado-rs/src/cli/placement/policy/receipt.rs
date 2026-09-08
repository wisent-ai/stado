//! What the operator is told: the delta between the policy the host carried
//! and the one it carries now, read back out of the host's own report.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::document::policy_document;
use super::install::apply_policy;
use super::{POLICY_DESTINATION, POLICY_FILE, POLICY_MARKER, PUBLISHED_BY, VANTAGE_MARKER};
use crate::cli::placement::candidates::{parse_registry, target};
use crate::cli::{host, CmdError};
use crate::deploy::production_runner;

/// One side of the change, as the host reported it.
struct PolicySnapshot {
    /// Registry generation the document was stamped with, or the script's word
    /// for a file that carried no stamp, could not be parsed, or was not there.
    generation: String,
    /// `true`, `false`, or `-` when no entry on that host named this machine.
    enabled: String,
    actions: Vec<String>,
}

/// Read one `PLACEMENT_POLICY <phase> ...` line out of the script's output.
///
/// The before-state arrives as remote output rather than as anything this
/// process knows, because the file it replaced only ever existed on that host.
/// A missing line is reported as missing and never defaulted to "the same as
/// now": defaulting would render every publication as a no-op and hide exactly
/// the drift this command exists to close.
fn snapshot(stdout: &str, phase: &str) -> Option<PolicySnapshot> {
    let prefix = format!("{POLICY_MARKER}\t{phase}\t");
    let line = stdout.lines().find(|line| line.starts_with(&prefix))?;
    let mut fields = line[prefix.len()..].split('\t');
    Some(PolicySnapshot {
        generation: fields.next()?.to_string(),
        enabled: fields.next()?.to_string(),
        actions: fields.next().map(action_list).unwrap_or_default(),
    })
}

/// `-` is the script's word for "no entry, or an empty list", not an action
/// named `-`.
fn action_list(field: &str) -> Vec<String> {
    if field == "-" {
        return Vec::new();
    }
    field
        .split(',')
        .filter(|action| !action.is_empty())
        .map(str::to_string)
        .collect()
}

/// Build and install one target's declared placement policy for
/// `stado route placement publish`.
///
/// The registry declares `weles.actions` per target and the worker never reads
/// it. The worker reads `~/.config/weles/placement-policy.json` on the box it
/// runs on. Those two disagreed: the registry listed `apple_create_developer_id`
/// and the host file did not, so the worker skipped the row in silence for hours
/// while the registry said it was allowed. Two sources of truth, and the one an
/// operator edits was not the one that decided.
///
/// This makes the host file a cache. It is generated from the registry, stamped
/// with `_source`, delivered over the audited channel, and refused on arrival if
/// the stamp is missing. Nothing here lets an operator put a list on a host that
/// the registry does not already declare — the only input is a target name.
///
/// It reports the delta rather than a success word. "Published" tells an
/// operator nothing; the generation it replaced and the actions that came and
/// went are the whole content of the operation, and an unchanged list is itself
/// an answer worth reading.
#[allow(clippy::too_many_lines)]
pub(crate) async fn publish_placement_policy_report(
    document: &Value,
    generation: &str,
    target_name: &str,
) -> Result<Value, CmdError> {
    let declared = parse_registry(document)?;
    let resolved = target(&declared, target_name)?.clone();
    let policy = policy_document(&resolved, generation, PUBLISHED_BY)?;

    // Staged as a file because the delivery channel carries files: the same
    // delivered-file path any other artifact takes, checksummed on arrival,
    // rather than a private scp with the audit trail removed.
    let staged = tempfile::Builder::new()
        .prefix("stado-placement-policy-")
        .suffix(".json")
        .tempfile()?;
    std::fs::write(
        staged.path(),
        format!("{}\n", serde_json::to_string_pretty(&policy)?),
    )?;
    let source = staged
        .path()
        .to_str()
        .ok_or_else(|| CmdError::click("the staged policy path is not valid UTF-8"))?;
    let (delivered, bytes) = host::deliver_file(&resolved.name, source, POLICY_FILE).await?;

    let runner = production_runner();
    let reported = apply_policy(&resolved, &runner).await.map_err(|error| {
        // Delivered and not installed is a real state, and the operator has
        // to be told which half happened: the worker is still running the
        // old list, and a file it does not read is sitting next to it. The
        // refusal names exactly which check the document failed.
        CmdError::click(format!(
            "{name}: the policy reached {delivered} and was NOT installed: {error}. \
                 Settle the refusal and publish again",
            name = resolved.name
        ))
    })?;

    let installed = snapshot(&reported, "installed").ok_or_else(|| {
        CmdError::click(format!(
            "{}: the apply step reported no installed policy, so {POLICY_DESTINATION} on that \
             host is now of unknown provenance; read it there before publishing again",
            resolved.name
        ))
    })?;
    let previous = snapshot(&reported, "previous");
    let vantage_prefix = format!("{VANTAGE_MARKER}\t");
    let vantage = reported
        .lines()
        .find_map(|line| line.strip_prefix(&vantage_prefix))
        .unwrap_or("-")
        .trim();

    // The delta is computed from what the host reports it now carries, not from
    // what was sent: the two are the same only if the script installed exactly
    // the document that was delivered, and that is the claim worth checking.
    let before: BTreeSet<&str> = previous
        .iter()
        .flat_map(|held| held.actions.iter().map(String::as_str))
        .collect();
    let after: BTreeSet<&str> = installed.actions.iter().map(String::as_str).collect();
    let added: Vec<&str> = after.difference(&before).copied().collect();
    let removed: Vec<&str> = before.difference(&after).copied().collect();
    let unchanged: Vec<&str> = after.intersection(&before).copied().collect();
    let current: Vec<&str> = installed.actions.iter().map(String::as_str).collect();
    let previous_generation = previous
        .as_ref()
        .map_or("unreported", |held| held.generation.as_str());
    let previous_enabled = previous
        .as_ref()
        .map_or("unreported", |held| held.enabled.as_str());

    Ok(json!({
        "target": resolved.name,
        "vantage": vantage,
        "delivered": delivered,
        "bytes": bytes,
        "installed": POLICY_DESTINATION,
        "registry_generation": generation,
        "previous_generation": previous_generation,
        "published_at": policy.pointer("/_source/published_at"),
        "enabled": installed.enabled,
        "previous_enabled": previous_enabled,
        "actions": current,
        "previous_actions": previous.as_ref().map(|held| held.actions.clone()),
        "added": added,
        "removed": removed,
        "unchanged": unchanged,
        "status": "published",
    }))
}
