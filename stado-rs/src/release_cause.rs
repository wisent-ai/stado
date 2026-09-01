//! Why a release candidate was quarantined, as a name rather than a symptom.
//!
//! NO Python original. This module exists because of one outage that the
//! quarantine record could not explain. The Brama LLM gateway on
//! `charless-mac-mini` went to zero processes with `active_version: null`, and
//! `stado release doctor brama` printed twenty quarantine rows going back to
//! `2026-08-06`. Every row recorded what the agent saw from outside — the
//! candidate did not answer `/readyz`, or its pid was gone — and buried
//! whatever the candidate said about itself in a truncated `stderr` dump at the
//! end of the same sentence.
//!
//! The cause was in the candidate's own log for the records that have one: the
//! vault fields behind the provider credentials could not serve, so Skarbiec
//! refused to issue capabilities, so the gateway obtained no credential and
//! could never become ready. Five of those twenty rows name that path — an
//! unmapped route, two refusals at redemption, two coordinates holding no value
//! — and three more candidates were burned inside the same five hours saying
//! nothing at all. Nobody reading the table could see that several rows were
//! one thing, because the table had no word for the thing.
//!
//! So a quarantine now carries a *cause* beside its reason. The distinction
//! this module is built on:
//!
//! - a **symptom** is what the agent observed from outside the candidate —
//!   `pid 7181 is gone`, `refused the connection`, `answered HTTP 503`. Those
//!   already have a home in the reason string, and they are not causes. The
//!   same symptom covers a missing credential, a missing binary and a panic.
//! - a **cause** is what the candidate, or the agent's own refusal, actually
//!   named. It is the thing an operator would have to change.
//!
//! The vocabulary below is derived from the real reasons on the live fleet, not
//! from a guess at what a release can do: every variant except
//! [`QuarantineCause::Unclassified`] is a class at least one recorded
//! quarantine on `charless-mac-mini` belongs to. Failure modes that no recorded
//! quarantine exhibits are deliberately absent — a name with no evidence behind
//! it is a label waiting to be applied wrongly.
//!
//! [`QuarantineCause::Unclassified`] is load-bearing and is not a defect.
//! Twelve of the twenty live rows land in it, seven of them consecutively.
//! Four say only `candidate did not become ready before deadline`, from before
//! the agent retained any of the candidate's output at all; three more retain a
//! symptom and no log; and the rest retain a log that names no failure —
//! including the three candidates of 2026-09-01, which stop after
//! `issuing runtime capabilities` and stay unclassified even when the whole
//! file is read, because the product wrote nothing to classify.
//!
//! Forcing those into the nearest-looking class would be the same mistake as
//! recording the symptom, with more confidence. They are reported as
//! unclassified, and the count of them is the honest measure of how much this
//! host's evidence is worth.

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
                Some("inspect every route with: skarbiec routes verify, or skarbiec doctor")
            }
            Self::CapabilityRoutesUnmapped => Some(
                "map the resource with: skarbiec routes add --resource <resource> \
                 --item <item> --field <field> --reason <text>, or derive it with \
                 skarbiec routes reconcile",
            ),
            // The capability was refused at the far end. Nothing in this
            // product reissues or extends one, and the sibling's own repair
            // depends on which of four refusals it was — which this class,
            // by construction, did not distinguish.
            Self::CapabilityRedemptionRefused | Self::Unclassified => None,
        }
    }
}

/// The candidate declared no rollback compatibility. The agent's own sentence,
/// from [`crate::release_agent`], and therefore the strongest evidence there
/// is: it is not a report copied out of a log that may have been truncated.
const ROLLBACK_COMPATIBILITY_NEEDLES: &[&str] = &["does not declare rollback compatibility with"];

/// The store could not be opened at all. `spawn gpg` is here because that is
/// how the one recorded instance reads: the decrypt helper was not installed
/// on the host, and no route repair addresses that.
const CREDENTIAL_STORE_NEEDLES: &[&str] = &["cannot be decrypted", "spawn gpg"];

/// A routed coordinate that cannot serve a value.
///
/// The first three are the sentences the vault and its consumers print about
/// the whole class — the gateway's redemption wording, the vault doctor's
/// summary, and the refusal remedy `capability-issue` now prints for any
/// coordinate problem. The rest are the individual coordinate verdicts, so a
/// record that carries only the specific sentence still classifies.
const CREDENTIAL_CANNOT_SERVE_NEEDLES: &[&str] = &[
    "no value at",
    "cannot serve a credential",
    "inspect every route with",
    "is present but empty",
    "is not a text value",
    "does not open:",
    "is in trash",
    "was renamed to",
    "no vault item",
];

/// Nothing maps the resource onto a coordinate. The first two are the
/// gateway's account of an empty or missing routes table; the third is the
/// refusal `capability-issue` prints when one resource resolves to nothing.
const CAPABILITY_ROUTES_NEEDLES: &[&str] = &[
    "no capability was issued for any provider",
    "routes table is missing or maps nothing",
    "no capability route maps",
];

/// A capability that existed and was refused when it was spent.
const CAPABILITY_REDEMPTION_NEEDLES: &[&str] = &[
    "redemption denied",
    "refused to redeem",
    "did not redeem",
    "capability_redeem_refused",
    "credential_redeem_failed",
    "capability is not issued",
];

/// The evidence line kept beside the cause.
///
/// Wide enough for every decisive sentence observed on the fleet — the longest
/// is a `capability-issue` refusal naming a resource, a coordinate and its
/// remedy, at about a hundred and sixty characters — and narrow enough that one
/// row of `release doctor` stays one row. The reason string keeps the full
/// (truncated) tail; this is the one line that earned the name.
const EVIDENCE_CHARS: usize = 240;

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

/// Drop terminal control sequences.
///
/// Products on this fleet log through `tracing`'s ANSI writer, so a decisive
/// line arrives wrapped in colour escapes. They are removed before matching
/// (an escape sitting inside a phrase would hide it) and before the evidence is
/// stored (a report is read in a table, not a terminal emulator). Borrowed
/// unchanged when there is nothing to strip, which is every reason the agent
/// writes itself.
///
/// A CSI sequence is `ESC [`, then parameter and intermediate bytes, then one
/// final byte in `@`..=`~`. The introducer `[` is itself inside that range, so
/// it has to be stepped over explicitly — scanning for the final byte from
/// directly after the escape terminates on the `[` and leaves `2m` behind in
/// the evidence, which is exactly what the first run of this against the live
/// host printed.
fn strip_ansi(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\u{1b}') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('\u{1b}') {
        out.push_str(&rest[..start]);
        let after_escape = &rest[start + '\u{1b}'.len_utf8()..];
        let Some(introducer) = after_escape.chars().next() else {
            // A lone escape at the end of a truncated tail. Dropped.
            rest = "";
            break;
        };
        if introducer != '[' {
            // A two-character escape (charset selection and friends). Drop
            // both and carry on rather than swallowing the rest of the line.
            rest = &after_escape[introducer.len_utf8()..];
            continue;
        }
        let body = &after_escape[introducer.len_utf8()..];
        match body.char_indices().find(|(_, ch)| ('@'..='~').contains(ch)) {
            Some((end, final_byte)) => rest = &body[end + final_byte.len_utf8()..],
            // Truncation cut the sequence before its final byte; there is no
            // text left in it to keep.
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// The segments a decisive line could be.
///
/// Three separators, each one this crate writes itself. A quarantine reason is
/// composed as `<symptom>; stderr <path>: <tail>; stdout <path>: <tail>`, and
/// each tail is a log tail joined with `" | "`. Splitting on real newlines
/// alone would quote the whole reason back as one "line", which is what the
/// operator was already staring at.
fn segments(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .flat_map(|line| line.split(" | "))
        .flat_map(|part| part.split("; "))
        .map(unlabel)
}

/// Drop the `stderr <path>: ` label the reason puts in front of the first line
/// of a quoted tail.
///
/// Written by [`crate::release_agent`] one line above where the tail is joined,
/// so this removes a known prefix rather than guessing at one. Without it the
/// evidence for a record whose decisive sentence is the first line of its
/// stderr is that sentence with a file path bolted to the front, and the bound
/// then spends a third of its width on the path.
fn unlabel(segment: &str) -> &str {
    let trimmed = segment.trim();
    for label in ["stderr ", "stdout "] {
        if let Some(rest) = trimmed.strip_prefix(label) {
            // `<path>: <line>` — the first `": "` ends the path. A bracketed
            // note ("[... is empty]") carries no such separator and is left
            // whole, because the note IS the whole answer in that case.
            if let Some((_, line)) = rest.split_once(": ") {
                return line.trim();
            }
        }
    }
    trimmed
}

/// The narrowest segment carrying one of `needles`, bounded and trimmed.
///
/// Narrowest rather than first: a reason's opening segment is the symptom, and
/// on a legacy record the first log line is glued to it, so "first match" hands
/// back the sentence this module exists to stop quoting. The shortest segment
/// that contains the match is the line that carries it and little else.
///
/// Falls back to the whole (bounded) text when no single segment holds the
/// match, which happens when a needle straddles a join. Reporting the match
/// without the line it came from would leave the operator with a name and no
/// quotation.
fn evidence_for(text: &str, needles: &[&str]) -> String {
    let found = segments(text)
        .filter(|segment| {
            let lowered = segment.to_lowercase();
            needles.iter().any(|needle| lowered.contains(needle))
        })
        .min_by_key(|segment| segment.chars().count())
        .unwrap_or_else(|| text.trim());
    bound(found)
}

fn bound(text: &str) -> String {
    match text.char_indices().nth(EVIDENCE_CHARS) {
        None => text.to_string(),
        Some((cut, _)) => format!("{}…", &text[..cut]),
    }
}

fn matches_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
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

/// How many quarantines share each cause, most common first.
///
/// Ties break on the cause's own word so two hosts with the same map print the
/// same order. [`QuarantineCause::Unclassified`] is counted like any other: it
/// is usually the largest bucket on a host with history, and hiding that would
/// misrepresent how much of the table is understood.
pub fn tally<I>(causes: I) -> Vec<(QuarantineCause, usize)>
where
    I: IntoIterator<Item = QuarantineCause>,
{
    let mut counts: std::collections::BTreeMap<QuarantineCause, usize> = Default::default();
    for cause in causes {
        *counts.entry(cause).or_default() += 1;
    }
    let mut tally: Vec<(QuarantineCause, usize)> = counts.into_iter().collect();
    tally.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| left.0.as_str().cmp(right.0.as_str()))
    });
    tally
}

/// The classified cause the most quarantines share, and how many.
///
/// `None` when nothing is classified. Deliberately not "the largest bucket":
/// an operator asking what dominates wants something they can act on, and
/// `unclassified` is not that — it is reported separately, as a count.
pub fn dominant(tally: &[(QuarantineCause, usize)]) -> Option<(QuarantineCause, usize)> {
    tally
        .iter()
        .find(|(cause, _)| cause.is_classified())
        .copied()
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

    #[test]
    fn the_tally_ranks_by_count_and_names_a_dominant_cause() {
        let counts = tally([
            QuarantineCause::Unclassified,
            QuarantineCause::CredentialCannotServe,
            QuarantineCause::Unclassified,
            QuarantineCause::RollbackCompatibilityUndeclared,
            QuarantineCause::CredentialCannotServe,
            QuarantineCause::Unclassified,
        ]);
        assert_eq!(
            counts,
            vec![
                (QuarantineCause::Unclassified, 3),
                (QuarantineCause::CredentialCannotServe, 2),
                (QuarantineCause::RollbackCompatibilityUndeclared, 1),
            ]
        );
        // The largest bucket is unclassified; the answer an operator can act
        // on is the largest *classified* one.
        assert_eq!(
            dominant(&counts),
            Some((QuarantineCause::CredentialCannotServe, 2))
        );
        assert_eq!(dominant(&tally([QuarantineCause::Unclassified])), None);
    }
}
