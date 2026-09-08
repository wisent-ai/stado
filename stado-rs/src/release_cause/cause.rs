//! The vocabulary itself: the named causes, their one spelling, and the
//! repair each one has when this fleet has one.

use serde::{Deserialize, Serialize};

/// The named cause behind one quarantine.
///
/// Spelled as a serialized enum, like [`crate::release_agent::RolloutPhase`]
/// and [`crate::release_control::QualificationStatus`], because it is stored in
/// the rollout state document and read back by an off-host command. The wire
/// words are `snake_case` for the same reason theirs are: the state file, the
/// published status row and the operator's terminal all print one spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineCause {
    /// The candidate does not declare rollback compatibility with the release
    /// it would replace, so the agent refused it before starting anything.
    ///
    /// The agent's own refusal rather than the product's report, and the only
    /// cause here that never involves reading a candidate log.
    RollbackCompatibilityUndeclared,
    /// The credential store itself could not be opened or decrypted on this
    /// host, so no credential behind it could be served at all.
    CredentialStoreUnreadable,
    /// A routed credential coordinate could not serve a value: the vault item
    /// is absent, renamed, trashed, will not open, carries no such field, or
    /// carries a field that is present and empty.
    ///
    /// One cause rather than seven because the sibling vault groups them under
    /// one check and repairs them with one command. This is the class the
    /// outage belonged to.
    CredentialCannotServe,
    /// No capability route maps the resource the candidate asked for onto any
    /// vault coordinate, so nothing could be issued for it.
    CapabilityRoutesUnmapped,
    /// A capability existed and the authority refused to redeem it — not
    /// issued, expired, out of uses, or an authorization id that did not
    /// match.
    ///
    /// Kept apart from [`Self::CredentialCannotServe`] even though the outage
    /// produced both, because the repair is not the same one and this class
    /// has no repair this product can offer.
    CapabilityRedemptionRefused,
    /// Nothing in the retained evidence names a cause.
    ///
    /// The default, so a record written before this field existed reads as
    /// "nobody classified this" rather than borrowing the first variant.
    #[default]
    Unclassified,
}

impl QuarantineCause {
    /// The word this cause is stored and printed as.
    ///
    /// Taken from the same serialization the state document carries, so the
    /// table, the JSON report and the file on the host cannot drift apart.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RollbackCompatibilityUndeclared => "rollback_compatibility_undeclared",
            Self::CredentialStoreUnreadable => "credential_store_unreadable",
            Self::CredentialCannotServe => "credential_cannot_serve",
            Self::CapabilityRoutesUnmapped => "capability_routes_unmapped",
            Self::CapabilityRedemptionRefused => "capability_redemption_refused",
            Self::Unclassified => "unclassified",
        }
    }

    /// Did the classifier actually name something?
    pub fn is_classified(self) -> bool {
        self != Self::Unclassified
    }

    /// The command or declaration that repairs this cause, when this fleet has
    /// one.
    ///
    /// `None` is a real answer and is returned more often than not. A verdict
    /// that classifies a failure and then invents an instruction is worse than
    /// one that names the cause and stops: the operator follows the invented
    /// instruction first. Every string here is a command that exists, or a
    /// declared manifest field that exists — the two credential sentences are
    /// the sibling vault's own remedy wording, copied rather than paraphrased
    /// so the two products tell an operator the same thing.
    pub fn remedy(self) -> Option<&'static str> {
        match self {
            Self::RollbackCompatibilityUndeclared => Some(
                "declare the active release in the candidate manifest's \
                 rollback_compatible_with, then promote it again",
            ),
            Self::CredentialStoreUnreadable => {
                Some("check which key can still open the vault with: stado credentials doctor")
            }
            Self::CredentialCannotServe => {
                Some("inspect every route with: skarbiec route verify, or skarbiec doctor")
            }
            Self::CapabilityRoutesUnmapped => Some(
                "map the resource with: skarbiec route declare --resource <resource> \
                 --item <item> --field <field> --reason <text>, and read what the vault \
                 already declares for itself with: skarbiec route resolve",
            ),
            // The capability was refused at the far end. Nothing in this
            // product reissues or extends one, and the sibling's own repair
            // depends on which of four refusals it was — which this class,
            // by construction, did not distinguish.
            Self::CapabilityRedemptionRefused | Self::Unclassified => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_legacy_record_without_a_cause_reads_as_unclassified() {
        // The live host's twenty records predate the field entirely.
        let record: QuarantineCause =
            serde_json::from_str("null").unwrap_or(QuarantineCause::Unclassified);
        assert_eq!(record, QuarantineCause::Unclassified);
        assert_eq!(QuarantineCause::default(), QuarantineCause::Unclassified);
    }

    #[test]
    fn the_wire_word_and_the_printed_word_are_one_word() {
        for cause in [
            QuarantineCause::RollbackCompatibilityUndeclared,
            QuarantineCause::CredentialStoreUnreadable,
            QuarantineCause::CredentialCannotServe,
            QuarantineCause::CapabilityRoutesUnmapped,
            QuarantineCause::CapabilityRedemptionRefused,
            QuarantineCause::Unclassified,
        ] {
            let wire = serde_json::to_value(cause).expect("cause serializes");
            assert_eq!(wire.as_str(), Some(cause.as_str()));
        }
    }

    #[test]
    fn only_causes_this_fleet_can_repair_carry_a_remedy() {
        assert!(QuarantineCause::CredentialCannotServe.remedy().is_some());
        assert!(QuarantineCause::CapabilityRoutesUnmapped.remedy().is_some());
        assert!(QuarantineCause::CredentialStoreUnreadable
            .remedy()
            .is_some());
        assert!(QuarantineCause::RollbackCompatibilityUndeclared
            .remedy()
            .is_some());
        // Named plainly, then stopped.
        assert!(QuarantineCause::CapabilityRedemptionRefused
            .remedy()
            .is_none());
        assert!(QuarantineCause::Unclassified.remedy().is_none());
    }
}
