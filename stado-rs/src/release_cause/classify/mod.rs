//! Reading a cause out of a recorded reason, deepest cause first.

mod decide;
mod envelope;
mod needles;
mod segments;

pub use decide::{classify, Classification};

pub(in crate::release_cause) use segments::bound;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release_cause::QuarantineCause;

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

    /// Read off charless-mac-mini on 2026-09-19: the desired Brama digest,
    /// quarantined while the host carried 3.6 GiB of swap at load 3.9. The
    /// record said `unclassified`, so the register blamed the candidate.
    const PROBE_UNANSWERED: &str = "active release lost readiness: \
        http://127.0.0.1:18081/readyz did not answer within 3s; stderr \
        /Users/charles/.stado/logs/brama-0.4.39.err: \u{1b}[2m2026-09-19T19:06:58.234319Z\u{1b}[0m \
        \u{1b}[33m WARN\u{1b}[0m credential_sign_in_blocked \
        blocked_by=\"subscription_identity_missing\"";

    #[test]
    fn a_probe_that_got_no_answer_is_its_own_class_with_a_host_remedy() {
        let found = classify(PROBE_UNANSWERED);
        assert_eq!(found.cause, QuarantineCause::ReadinessProbeUnanswered);
        assert!(
            found.evidence.contains("did not answer within 3s"),
            "quoted {:?}",
            found.evidence
        );
        let remedy = QuarantineCause::ReadinessProbeUnanswered
            .remedy()
            .unwrap_or_default();
        assert!(remedy.contains("stado space report"), "{remedy}");
    }

    /// A candidate that answered, or one whose process was gone, is not a
    /// host that could not run it: the deeper sentence in the same record
    /// still decides.
    #[test]
    fn an_answered_probe_keeps_the_cause_its_evidence_names() {
        assert_eq!(
            classify(OUTAGE_CREDENTIAL).cause,
            QuarantineCause::CredentialCannotServe
        );
        assert_eq!(
            classify(REDEMPTION_ONLY).cause,
            QuarantineCause::CapabilityRedemptionRefused
        );
        assert_eq!(classify(NO_EVIDENCE).cause, QuarantineCause::Unclassified);
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
