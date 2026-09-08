//! Reading a cause out of a recorded reason, deepest cause first.

mod needles;
mod segments;

use super::cause::QuarantineCause;
use needles::{
    CAPABILITY_REDEMPTION_NEEDLES, CAPABILITY_ROUTES_NEEDLES, CREDENTIAL_CANNOT_SERVE_NEEDLES,
    CREDENTIAL_STORE_NEEDLES, ROLLBACK_COMPATIBILITY_NEEDLES,
};
use segments::{evidence_for, matches_any, strip_ansi};

pub(in crate::release_cause) use segments::bound;

/// The cause a quarantine's evidence names, and the exact line it was read
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub cause: QuarantineCause,
    /// The one line the cause was taken from, ANSI-stripped and bounded.
    /// Empty exactly when the cause is [`QuarantineCause::Unclassified`].
    pub evidence: String,
}

impl Classification {
    fn unclassified() -> Self {
        Self {
            cause: QuarantineCause::Unclassified,
            evidence: String::new(),
        }
    }
}

/// Name the cause behind one quarantine, from the reason and whatever of the
/// candidate's own output is available.
///
/// Order encodes causality, not convenience, and it is the whole correctness
/// argument of this function. The recorded outage produced a log naming both
/// `no value at provider:kimi:…#value` and `capability is not issued`: the
/// empty vault field is why the capability was refused, so a classifier that
/// checked redemption first would have reported the consequence and sent the
/// operator to the capability lifecycle instead of the credential.
///
/// So the deepest thing the evidence can name wins:
///
/// 1. the agent's own refusal, which no log can contradict;
/// 2. the store not opening, below which nothing can work;
/// 3. a routed coordinate that cannot serve;
/// 4. no route at all;
/// 5. a capability refused when it was spent — last, because every cause above
///    can produce this sentence as a symptom.
///
/// Anything else is [`QuarantineCause::Unclassified`], with no evidence line:
/// there is no honest sentence to quote.
pub fn classify(text: &str) -> Classification {
    let clean = strip_ansi(text);
    let haystack = clean.to_lowercase();
    for (needles, cause) in [
        (
            ROLLBACK_COMPATIBILITY_NEEDLES,
            QuarantineCause::RollbackCompatibilityUndeclared,
        ),
        (
            CREDENTIAL_STORE_NEEDLES,
            QuarantineCause::CredentialStoreUnreadable,
        ),
        (
            CREDENTIAL_CANNOT_SERVE_NEEDLES,
            QuarantineCause::CredentialCannotServe,
        ),
        (
            CAPABILITY_ROUTES_NEEDLES,
            QuarantineCause::CapabilityRoutesUnmapped,
        ),
        (
            CAPABILITY_REDEMPTION_NEEDLES,
            QuarantineCause::CapabilityRedemptionRefused,
        ),
    ] {
        if matches_any(&haystack, needles) {
            return Classification {
                cause,
                evidence: evidence_for(&clean, needles),
            };
        }
    }
    Classification::unclassified()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim reasons read read-only off `charless-mac-mini` with
    /// `stado release quarantine list --json`, one per class the live data
    /// exhibits, ANSI escapes and truncation included.
    ///
    /// These are fixtures rather than a live call on purpose: the host's map
    /// changes as the agent runs, and a classifier whose test moves with
    /// production tests nothing.
    const OUTAGE_CREDENTIAL: &str = "candidate did not become ready within 90s: \
        http://127.0.0.1:18081/readyz answered HTTP 503 Service Unavailable; stderr \
        /Users/charles/.stado/logs/brama-0.2.55.err: \u{1b}[2m2026-09-01T22:43:04.128714Z\u{1b}[0m \
        \u{1b}[33m WARN\u{1b}[0m \u{1b}[2mbrama::subscription_dispatch::refresh_sweep\u{1b}[0m: \
        skarbiec: redemption denied: no value at \
        provider:kimi:brama-sub-wisent-app-kimi-primary#value for resource provider:kimi | \
        2026-09-01T22:43:39.245320Z  WARN…; stdout [is empty]";

    const REDEMPTION_ONLY: &str = "candidate did not become ready within 90s: \
        http://127.0.0.1:18081/health refused the connection; stderr \
        /Users/charles/.stado/logs/brama-0.2.43.err: the authority refused to redeem this \
        capability event=\"subscription_capability_redeem_refused\" \
        resource=provider:codex:brama-sub-wisent-app-codex-primary error=capability \
        redemption denied";

    const ROUTES_UNMAPPED: &str = "candidate did not become ready within 90s: pid 7181 is gone; \
        stderr /Users/charles/.stado/logs/brama-0.2.36.err: Caused by: | trailing characters at \
        line 1244434 column 2 | skipping subscription agent:wisent-app | no capability was issued \
        for any provider on this host: the routes table is missing or maps nothing. | Expected \
        at: /Users/charles/.stado/capability-routes.json";

    const STORE_UNREADABLE: &str = "candidate did not become ready within 90s: \
        http://127.0.0.1:18895/readyz answered HTTP 503 Service Unavailable; stderr \
        /Users/charles/.stado/logs/skarbiec-0.2.33.err: skarbiec API listening on \
        http://127.0.0.1:18895 (loopback only) | skarbiec readiness monitor: stored item \
        0dfed61c-eae2-4a81-98a1-0e5763c37498 cannot be decrypted: spawn gpg: No such file or \
        directory (os error 2)";

    const ROLLBACK_COMPAT: &str = "release 0.2.54 does not declare rollback compatibility \
        with 0.2.53";

    /// The pre-instrumentation format: seven of the live rows say only this.
    const NO_EVIDENCE: &str = "candidate did not become ready before deadline";

    /// A symptom with nothing behind it. The point of the unclassified bucket.
    const SYMPTOM_ONLY: &str = "candidate did not become ready within 90s: \
        http://127.0.0.1:18080/health refused the connection";

    #[test]
    fn empty_vault_field_beats_the_refusal_it_caused() {
        // The outage record names both. The empty field is why the capability
        // was refused, so reporting `capability_redemption_refused` here would
        // send the operator to the wrong subsystem.
        let found = classify(OUTAGE_CREDENTIAL);
        assert_eq!(found.cause, QuarantineCause::CredentialCannotServe);
        assert!(
            found.evidence.contains("no value at"),
            "evidence must quote the decisive line, got {:?}",
            found.evidence
        );
        assert!(
            !found.evidence.contains('\u{1b}'),
            "evidence must not carry terminal escapes, got {:?}",
            found.evidence
        );
    }

    #[test]
    fn a_colour_escape_leaves_nothing_of_itself_behind() {
        // `[` is inside the `@`..=`~` final-byte range, so a scan that starts
        // one character after the escape stops on the introducer and leaks the
        // parameter bytes. The live host printed `2m2026-08-27T...0m 33m WARN0m`
        // through exactly that hole.
        let wrapped = "\u{1b}[2m2026-08-27T18:57:11Z\u{1b}[0m \u{1b}[33m WARN\u{1b}[0m \
                       \u{1b}[2mbrama::gateway::broker\u{1b}[0m: the authority refused to \
                       redeem this capability";
        let found = classify(wrapped);
        assert_eq!(found.cause, QuarantineCause::CapabilityRedemptionRefused);
        assert_eq!(
            found.evidence,
            "2026-08-27T18:57:11Z  WARN brama::gateway::broker: the authority refused to \
             redeem this capability"
        );
    }

    #[test]
    fn evidence_drops_the_stream_label_the_reason_added() {
        // The decisive sentence is the first line of the quoted tail, so it
        // arrives glued to `stderr <path>: `. Quoting the path back would spend
        // a third of the width on it.
        let found = classify(
            "candidate did not become ready within 90s: pid 7181 is gone; stderr \
             /Users/charles/.stado/logs/brama-0.2.36.err: no capability was issued for any \
             provider on this host: the routes table is missing or maps nothing.",
        );
        assert_eq!(found.cause, QuarantineCause::CapabilityRoutesUnmapped);
        assert_eq!(
            found.evidence,
            "no capability was issued for any provider on this host: the routes table is \
             missing or maps nothing."
        );
    }

    #[test]
    fn evidence_is_the_decisive_line_not_the_symptom_in_front_of_it() {
        // "First segment containing the match" returns the symptom sentence
        // whenever the reason prefix and the first log line share a segment,
        // which is the whole reason an operator could not read this table.
        let found = classify(REDEMPTION_ONLY);
        assert_eq!(found.cause, QuarantineCause::CapabilityRedemptionRefused);
        assert!(
            !found.evidence.contains("did not become ready"),
            "evidence quoted the symptom instead of the cause: {:?}",
            found.evidence
        );
    }

    #[test]
    fn a_refusal_with_no_deeper_cause_is_named_as_the_refusal() {
        assert_eq!(
            classify(REDEMPTION_ONLY).cause,
            QuarantineCause::CapabilityRedemptionRefused
        );
    }

    #[test]
    fn each_live_class_gets_its_own_name() {
        for (text, expected) in [
            (ROUTES_UNMAPPED, QuarantineCause::CapabilityRoutesUnmapped),
            (STORE_UNREADABLE, QuarantineCause::CredentialStoreUnreadable),
            (
                ROLLBACK_COMPAT,
                QuarantineCause::RollbackCompatibilityUndeclared,
            ),
        ] {
            assert_eq!(classify(text).cause, expected, "misread {text:?}");
        }
    }

    #[test]
    fn a_symptom_is_never_dressed_up_as_a_cause() {
        for text in [NO_EVIDENCE, SYMPTOM_ONLY, ""] {
            let found = classify(text);
            assert_eq!(
                found.cause,
                QuarantineCause::Unclassified,
                "invented a cause for {text:?}"
            );
            assert!(
                found.evidence.is_empty(),
                "quoted evidence it does not have"
            );
        }
    }

    #[test]
    fn evidence_quotes_one_line_not_the_whole_tail() {
        let found = classify(ROUTES_UNMAPPED);
        assert_eq!(
            found.evidence,
            "no capability was issued for any provider on this host: the routes table is \
             missing or maps nothing."
        );
    }
}
