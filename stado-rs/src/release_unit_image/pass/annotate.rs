//! What `registry doctor` appends to a `stale-unit-image` row, read once per
//! local doctor pass.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::deploy::service::{ImageState, StaleUnitImage};

use crate::release_unit_image::ledger::attempt::RevisitAttempt;
use crate::release_unit_image::ledger::{load_ledger, RevisitLedger};
use crate::release_unit_image::registry_policy::contract::validate_registry_contract;
use crate::release_unit_image::registry_policy::policy;
use crate::release_unit_image::registry_policy::scope::{declared_units, host_scope};

/// Everything `registry doctor` needs to annotate its `stale-unit-image` rows,
/// read ONCE per local doctor pass.
///
/// Built before the row loop rather than inside it: the ledger is one file and
/// one host-wide document, so opening it per row would be one read per finding
/// for one answer.
pub(crate) struct RevisitAnnotations {
    /// Every label the document authorises on this target, whether or not the
    /// contract resolved.
    declared: BTreeMap<String, String>,
    /// The state the agent will consult, or why it cannot be read.
    ///
    /// A `Result` and never an `Option`, because the failure is the finding: a
    /// ledger the agent cannot read is a host where the revisit pass will not
    /// act, and a doctor that dropped the reason would report the stale unit
    /// while silently omitting why nothing is coming for it. That is the exact
    /// defect — an unread state rendered as a clean one — that this whole
    /// check exists to remove.
    state: Result<RevisitLedger, String>,
}

/// The annotations for one host, or `None` when there is nothing to annotate.
///
/// No clause is added when the target declares no image revisit policy.
/// `local_units` is the host this process runs on: the ledger is a local file,
/// so a row about another host gets no clause.
pub(crate) fn annotations(
    document: &Value,
    target_name: &str,
    local_units: Option<&str>,
) -> Option<RevisitAnnotations> {
    if local_units != Some(target_name) {
        return None;
    }
    // A malformed policy cannot identify an authorised label reliably, so it
    // has no unit clause; `builds_refusing_registry` still reports the parser's
    // exact refusal through the existing `build-refuses-registry` finding.
    let policy = policy(document).ok().flatten()?;
    let declared = declared_units(&policy, target_name);
    if declared.is_empty() {
        return None;
    }
    // Refuse to open the ledger until the whole policy contract is valid. A
    // parseable policy still identifies its authorised labels, so an unsafe
    // path, platform mismatch or ownership contradiction is stated on those
    // rows as unresolved state rather than silently disappearing.
    let state = match validate_registry_contract(document) {
        Err(reason) => Err(reason),
        Ok(()) => match host_scope(&policy, target_name) {
            Ok(Some(scope)) => load_ledger(&scope.state_dir, target_name),
            Ok(None) => {
                Err("no product resolves a revisit state directory for this host".to_string())
            }
            Err(reason) => Err(reason),
        },
    };
    Some(RevisitAnnotations { declared, state })
}

impl RevisitAnnotations {
    /// The clause to append to one `stale-unit-image` row.
    ///
    /// Only for a unit some policy explicitly authorises on this target. A row
    /// about a unit no product named gets no clause, because the agent neither
    /// tried nor may try it, and saying otherwise would be a claim about a
    /// unit outside this feature.
    pub(crate) fn clause(&self, image: &StaleUnitImage) -> Option<String> {
        if !self.declared.contains_key(&image.unit) {
            return None;
        }
        let (running, declared) = match &image.state {
            ImageState::Unlinked { running, installed }
            | ImageState::Replaced { running, installed } => (running, installed),
            ImageState::Unread { .. } => return None,
        };
        match &self.state {
            Err(reason) => Some(format!(
                ". This unit is authorised in release_unit_image_revisit, so the release agent \
                 would \
                 normally restart it, but this host's revisit state could not be read — so \
                 whether it has already been attempted is unknown and the agent will not act \
                 until that is resolved: {reason}"
            )),
            Ok(ledger) => ledger
                .barring(&image.unit, running, declared)
                .map(RevisitAttempt::clause),
        }
    }
}
