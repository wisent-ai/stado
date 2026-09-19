//! Reading a cause out of the failure envelope the emitting service wrote,
//! rather than out of its English.
//!
//! Every Wisent service logs a refusal as the `wisent-errors` envelope:
//! `{"failure_point": "<dotted path>", "error_code": "<catalogue code>",
//! …, "detail": "…", "cause": {…}}`. Both keys are declared vocabulary — the
//! point by the emitting product, the code by the shared catalogue — so a
//! classifier keyed to them survives every rewording of the sentence.
//!
//! The sentences in [`super::needles`] do not. On 2026-09-19 the operator
//! asked where keyword logic decides things in this codebase and how it
//! should be repaired; `release doctor brama` was one of the answers, because
//! it reads a cause by looking for English in a log tail. The same day it
//! reported `capability_routes_unmapped` from evidence its own register had
//! truncated to `no capability route maps reso…`, with the resource name —
//! the one thing an operator needs — cut off, while the envelope beside it
//! carried that resource in `detail`.
//!
//! The envelope wins when it is there; the sentences remain for a log line
//! written before a product adopted the package.

use serde_json::Value;
use wisent_errors::Code;

use super::super::cause::QuarantineCause;

/// The last segment of a failure point: the operation that refused. Products
/// prefix their points with service and layer (`brama.gateway.…`), and the
/// depth carries no meaning by contract, so the operation is what is read.
fn operation(point: &str) -> &str {
    point.rsplit('.').next().unwrap_or(point)
}

/// What one envelope means for a rollout, from the operation that refused and
/// the catalogue code it refused with.
///
/// Both halves are needed: one operation refuses for several reasons.
/// `credential-redeem` with `config` is a routing table that names nothing,
/// and with `auth` it is a capability the authority would not honour — the
/// two the fleet met on the same host within one minute.
fn cause_of(point: &str, code: Code) -> Option<QuarantineCause> {
    match (operation(point), code) {
        ("credential-redeem" | "subscription-load", Code::Config) => {
            Some(QuarantineCause::CapabilityRoutesUnmapped)
        }
        ("credential-redeem" | "capability-issue", Code::Auth | Code::Refused) => {
            Some(QuarantineCause::CapabilityRedemptionRefused)
        }
        ("credential-read" | "vault-open" | "credential-redeem", Code::InfraDown) => {
            Some(QuarantineCause::CredentialStoreUnreadable)
        }
        ("credential-read" | "vault-open", Code::NotFound | Code::Config) => {
            Some(QuarantineCause::CredentialCannotServe)
        }
        _ => None,
    }
}

/// One envelope's classification: the cause it names and the sentence it
/// carries for the operator.
pub(super) struct Named {
    pub(super) cause: QuarantineCause,
    pub(super) evidence: String,
}

/// Every JSON object in `text` that carries a failure point, outermost first.
///
/// A log line puts the envelope after a field name (`envelope={…}`) or on its
/// own, so the scan is for balanced braces rather than for a whole line that
/// parses. Depth is tracked outside strings; a brace inside a quoted detail
/// does not close the object.
fn objects(text: &str) -> Vec<Value> {
    let bytes: Vec<char> = text.chars().collect();
    let mut found = Vec::new();
    let mut index = usize::default();
    while index < bytes.len() {
        if bytes[index] != '{' {
            index += usize::from(true);
            continue;
        }
        let mut depth = usize::default();
        let mut quoted = false;
        let mut escaped = false;
        let mut end = None;
        for (offset, ch) in bytes[index..].iter().enumerate() {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' if quoted => escaped = true,
                '"' => quoted = !quoted,
                '{' if !quoted => depth += usize::from(true),
                '}' if !quoted => {
                    depth -= usize::from(true);
                    if depth == usize::default() {
                        end = Some(index + offset + usize::from(true));
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        let candidate: String = bytes[index..end].iter().collect();
        if let Ok(value) = serde_json::from_str::<Value>(&candidate) {
            if value.get("failure_point").is_some() {
                found.push(value);
            }
        }
        index = end;
    }
    found
}

/// The deepest cause of one envelope: a nested `cause` explains the layer
/// above it, which is the same rule [`super::classify`] encodes for sentences.
fn deepest(envelope: &Value) -> &Value {
    let mut node = envelope;
    while let Some(cause) = node.get("cause").filter(|value| value.is_object()) {
        node = cause;
    }
    node
}

/// The named cause and evidence of one envelope's fields.
fn named(point: &str, code: Code, detail: &str) -> Option<Named> {
    let cause = cause_of(point, code)?;
    let detail = detail.trim();
    let evidence = if detail.is_empty() {
        format!("{point} [{}]", code.as_str())
    } else {
        format!("{point} [{}] {detail}", code.as_str())
    };
    Some(Named {
        cause,
        evidence: super::bound(&evidence),
    })
}

/// One string field of a JSON object, read out of the raw text.
///
/// For an envelope a log tail cut in half: the keys are written before the
/// long `detail`, so they survive a truncation that stops the document from
/// parsing. Every quarantine record on charless-mac-mini carrying the
/// 2026-09-18 routing failure is in exactly that shape — the register's own
/// bound landed inside `detail`.
fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let opening = format!("\"{key}\":");
    let start = text.find(&opening)? + opening.len();
    let rest = text[start..].trim_start();
    let rest = rest.strip_prefix('"')?;
    match rest.find('"') {
        Some(end) => Some(&rest[..end]),
        // The value itself was cut: what is there is still what it said.
        None => Some(rest),
    }
}

/// The cause the envelopes in `text` name, if any of them names one this
/// rollout knows. The evidence is the envelope's own `detail`.
pub(super) fn classify(text: &str) -> Option<Named> {
    for envelope in objects(text) {
        let node = deepest(&envelope);
        for candidate in [node, &envelope] {
            let point = candidate
                .get("failure_point")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(code) = candidate
                .get("error_code")
                .and_then(Value::as_str)
                .and_then(Code::parse)
            else {
                continue;
            };
            let detail = candidate
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(found) = named(point, code, detail) {
                return Some(found);
            }
        }
    }
    // No object closed: the tail was cut inside one. Read its keys anyway.
    let start = text.find("\"failure_point\":")?;
    let cut = &text[start..];
    let point = field(cut, "failure_point")?;
    let code = Code::parse(field(cut, "error_code")?)?;
    named(point, code, field(cut, "detail").unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::{classify, objects};
    use crate::release_cause::cause::QuarantineCause;

    /// The line Brama logged on charless-mac-mini on 2026-09-18, shortened
    /// only where the resource list repeats.
    const LIVE: &str = "2026-09-18T21:12:20.988678Z  WARN brama::gateway::broker: our deployment \
        configuration is incomplete or wrong — our failure; retrying will not help \
        {\"failure_point\":\"brama.gateway.credential-redeem\",\"error_code\":\"config\",\
        \"service\":\"brama\",\"impact\":\"one credential lookup\",\"severity\":\"critical\",\
        \"retryable\":false,\"outage\":true,\"detail\":\"no capability route maps resource \
        `provider:claude-code`: 4 vault items declare provider:claude-code\"}";

    #[test]
    fn the_envelope_names_the_cause_and_quotes_the_resource_the_register_truncated() {
        let named = classify(LIVE).expect("classified");
        assert_eq!(named.cause, QuarantineCause::CapabilityRoutesUnmapped);
        assert!(
            named.evidence.contains("provider:claude-code"),
            "{}",
            named.evidence
        );
        assert!(
            named.evidence.contains("brama.gateway.credential-redeem"),
            "{}",
            named.evidence
        );
    }

    #[test]
    fn a_nested_cause_outranks_the_layer_that_reported_it() {
        let text = "{\"failure_point\":\"brama.gateway.credential-redeem\",\
            \"error_code\":\"config\",\"detail\":\"no capability route maps resource\",\
            \"cause\":{\"failure_point\":\"brama.gateway.credential-redeem\",\
            \"error_code\":\"auth\",\"detail\":\"capability issuance refused\"}}";
        let named = classify(text).expect("classified");
        assert_eq!(named.cause, QuarantineCause::CapabilityRedemptionRefused);
        assert!(named.evidence.contains("capability issuance refused"));
    }

    #[test]
    fn a_brace_inside_a_detail_does_not_end_the_envelope() {
        let text = "{\"failure_point\":\"brama.gateway.credential-redeem\",\
            \"error_code\":\"config\",\"detail\":\"the body was {\\\"a\\\": 1} and it refused\"}";
        assert_eq!(objects(text).len(), usize::from(true));
        assert!(classify(text)
            .expect("classified")
            .evidence
            .contains("and it refused"));
    }

    #[test]
    fn a_line_with_no_envelope_and_an_unknown_operation_name_nothing() {
        assert!(classify("candidate did not become ready before deadline").is_none());
        let other = "{\"failure_point\":\"brama.plan.usage-report\",\"error_code\":\"timeout\",\
            \"detail\":\"the provider did not answer\"}";
        assert!(classify(other).is_none());
    }

    /// Verbatim from `stado release quarantine list brama --json` on
    /// 2026-09-19, digest 5fda0ea8…: the register's own bound landed inside
    /// `detail`, so no object closes and the resource name is half there.
    /// The record still says what broke, and the half sentence still beats
    /// quoting the blob.
    #[test]
    fn a_register_record_cut_inside_its_envelope_still_names_the_cause() {
        let cut = "candidate did not become ready within 90s; stderr brama-0.4.24.err: \
            retrying will not help {\"failure_point\":\"brama.gateway.credential-redeem\",\
            \"error_code\":\"config\",\"service\":\"brama\",\"impact\":\"one credential lookup\",\
            \"severity\":\"critical\",\"retryable\":false,\"outage\":true,\
            \"detail\":\"no capability route maps reso";
        let named = classify(cut).expect("classified");
        assert_eq!(named.cause, QuarantineCause::CapabilityRoutesUnmapped);
        assert!(
            named
                .evidence
                .starts_with("brama.gateway.credential-redeem [config]"),
            "{}",
            named.evidence
        );
        assert!(
            !named.evidence.contains("retrying will not help"),
            "{}",
            named.evidence
        );
    }
}
