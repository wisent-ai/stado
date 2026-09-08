//! Asking whether the condition a cause names is still true, instead of
//! inferring it from how often the same quarantine came back.

mod routes_verify;

use super::cause::QuarantineCause;

pub use routes_verify::{read_routes_verify, routes_verify_detail};

/// What a cause's predicate observed about the wall, right now.
///
/// Three values, not two, and the third is the point. Counting quarantines
/// infers "unchanged" from repetition, which is a fair proxy for a cause that
/// leaves no trace to inspect and a bad one for a standing external
/// precondition: an empty vault field refilled only by a human browser sign-in
/// does not become more true by being observed three times. Asking the
/// condition replaces the proxy — but only when the answer is actually an
/// answer, so "I could not tell" is a value here rather than a silent `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallVerdict {
    /// The predicate ran and the wall is still standing.
    Present,
    /// The predicate ran and the condition it names is satisfied.
    Gone,
    /// The predicate could not answer: no binary, no table, a vault that will
    /// not open, a timeout, an unreadable report, or a scope that matched
    /// nothing. Never permission to promote and never grounds to refuse on its
    /// own — the caller falls back to counting.
    Unknown,
}

/// The read-only command that answers whether one cause's wall still stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CausePredicate {
    /// Arguments after the `skarbiec` binary.
    pub args: Vec<String>,
    /// The resource the check was scoped to, for the refusal to quote.
    pub resource: String,
}

/// The resource coordinate an evidence line names, if it names one.
///
/// Two wordings, both real. `no value at <resource>#<field> for resource <r>`
/// is what the live gateway logged for the two records that carry this cause;
/// `capability-issue refused for <resource>: …` is what the sibling's merged
/// refusal prints, so records written from here on carry that instead. The
/// coordinate before `#` is preferred because it is the routes table's own key:
/// the trailing `for resource provider:kimi` is the provider family, and
/// scoping to it would drag in every sibling subscription.
fn resource_in(evidence: &str) -> Option<&str> {
    let exact = |token: &str| !token.is_empty() && !token.contains(char::is_whitespace);
    if let Some(rest) = evidence.split("no value at ").nth(1) {
        let token = rest
            .split(['#', ' '])
            .next()
            .unwrap_or_default()
            .trim_end_matches([',', ';', '.']);
        if exact(token) {
            return Some(token);
        }
    }
    if let Some(rest) = evidence.split("refused for ").nth(1) {
        let token = rest
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches([':', ',', ';', '.']);
        if exact(token) {
            return Some(token);
        }
    }
    None
}

impl QuarantineCause {
    /// The command that answers whether this cause's wall still stands, when
    /// this fleet has one and the evidence says what to ask about.
    ///
    /// Only [`Self::CredentialCannotServe`] has one today, and it is the
    /// command already named as that cause's remedy: `skarbiec routes verify`
    /// exits non-zero exactly when a route cannot serve a usable credential.
    /// Attaching a remedy and never calling it was the gap this closes.
    ///
    /// `None` when the evidence names no resource, and that is deliberate. The
    /// check takes a substring scope, so an unscoped run would report any
    /// unrelated broken route in the table as this candidate's wall and refuse
    /// a promotion that had nothing to do with it. No scope, no predicate, fall
    /// back to counting.
    ///
    /// The other causes have no predicate on purpose: a missing `gpg`, an
    /// undeclared rollback compatibility and a refused redemption are not
    /// conditions this crate can re-ask cheaply and read-only.
    pub fn predicate(self, evidence: &str) -> Option<CausePredicate> {
        if self != Self::CredentialCannotServe {
            return None;
        }
        let resource = resource_in(evidence)?;
        Some(CausePredicate {
            args: vec![
                "routes".to_string(),
                "verify".to_string(),
                resource.to_string(),
            ],
            resource: resource.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_predicate_scopes_to_the_resource_the_evidence_names() {
        // The live evidence line for b54ea076, the desired digest.
        let evidence = "skarbiec: redemption denied: no value at \
                        provider:kimi:brama-sub-wisent-app-kimi-primary#value for resource \
                        provider:kimi";
        let predicate = QuarantineCause::CredentialCannotServe
            .predicate(evidence)
            .expect("this cause has a predicate and this evidence names a resource");
        assert_eq!(
            predicate.resource,
            "provider:kimi:brama-sub-wisent-app-kimi-primary"
        );
        // The coordinate, not the trailing provider family: scoping to
        // `provider:kimi` would drag in every sibling subscription.
        assert_eq!(
            predicate.args,
            vec![
                "routes",
                "verify",
                "provider:kimi:brama-sub-wisent-app-kimi-primary"
            ]
        );
    }

    #[test]
    fn the_siblings_merged_refusal_wording_also_yields_a_scope() {
        let evidence = "capability-issue refused for provider:openai: vault item provider-openai \
                        field api_key is present but empty; inspect every route with: skarbiec \
                        routes verify, or skarbiec doctor";
        let predicate = QuarantineCause::CredentialCannotServe
            .predicate(evidence)
            .expect("the sibling's refusal names its resource");
        assert_eq!(predicate.resource, "provider:openai");
    }

    #[test]
    fn no_resource_in_the_evidence_means_no_predicate() {
        // An unscoped run would report an unrelated broken route as this
        // candidate's wall. Better to fall back to counting.
        assert!(QuarantineCause::CredentialCannotServe
            .predicate("vault item X field value is present but empty")
            .is_none());
        assert!(QuarantineCause::CredentialCannotServe
            .predicate("")
            .is_none());
    }

    #[test]
    fn causes_without_a_checkable_condition_have_no_predicate() {
        for cause in [
            QuarantineCause::RollbackCompatibilityUndeclared,
            QuarantineCause::CredentialStoreUnreadable,
            QuarantineCause::CapabilityRedemptionRefused,
            QuarantineCause::CapabilityRoutesUnmapped,
            QuarantineCause::Unclassified,
        ] {
            assert!(
                cause.predicate("no value at provider:kimi#value").is_none(),
                "{} must not borrow another cause's predicate",
                cause.as_str()
            );
        }
    }
}
