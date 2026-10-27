//! Reading a cause out of the failure envelope the emitting service wrote,
//! rather than out of its English.
//!
//! Every Wisent service logs a refusal as the `wisent-errors` envelope:
//! `{"failure_point": "<dotted path>", "error_code": "<catalogue code>",
//! …, "detail": "…", "cause": {…}}`. Both keys are declared vocabulary — the
//! point by the emitting product, the code by the shared catalogue — so a
//! classifier keyed to them survives every rewording of the sentence.
//!
//! A log line without an envelope names no cause: the envelope carries the
//! resource or coordinate in `detail` whole, where a sentence can be cut off
//! by the register's bound.

use serde_json::Value;
use wisent_errors::Code;

use super::super::cause::QuarantineCause;

/// The last segment of a failure point: the operation that refused. Products
/// prefix their points with service and layer (`brama.gateway.…`), and the
/// depth carries no meaning by contract, so the operation is what is read.
fn operation(point: &str) -> &str {
    // The segment after the last dot; a point with no dots is one segment.
    match point.rsplit_once('.') {
        Some((_prefix, operation)) => operation,
        None => point,
    }
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
/// own, so each `{` is tried as the start of one JSON value; serde's stream
/// reader says where that value ended, and the scan resumes there. A `{` that
/// starts no valid value is stepped over.
fn objects(text: &str) -> Vec<Value> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let tail = &rest[start..];
        let mut stream = serde_json::Deserializer::from_str(tail).into_iter::<Value>();
        match stream.next() {
            Some(Ok(value)) => {
                rest = &tail[stream.byte_offset()..];
                if value.get("failure_point").is_some() {
                    found.push(value);
                }
            }
            // The `{` starts no JSON value (serde refused it, or the text
            // ended): log text around an envelope is not JSON, so this is the
            // scan's answer, not a failure. Step over the `{`, which `tail`
            // starts with, and scan on.
            _ => rest = &tail['{'.len_utf8()..],
        }
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
        evidence: super::evidence_line(&evidence),
    })
}

/// One string field of a JSON object, read out of the raw text.
///
/// For an envelope a log tail cut in half: the keys are written before the
/// long `detail`, so they survive a truncation that stops the document from
/// parsing. A quarantine record whose register bound lands inside `detail` is
/// in exactly that shape.
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
            // A nested cause may carry only a code; the point is then the
            // empty operation, which names no cause, and the outer envelope
            // is tried next. A detail the envelope does not carry is no text.
            let point = match candidate.get("failure_point").and_then(Value::as_str) {
                Some(point) => point,
                None => "",
            };
            let Some(code) = candidate
                .get("error_code")
                .and_then(Value::as_str)
                .and_then(Code::parse)
            else {
                continue;
            };
            let detail = match candidate.get("detail").and_then(Value::as_str) {
                Some(detail) => detail,
                None => "",
            };
            if let Some(found) = named(point, code, detail) {
                return Some(found);
            }
        }
    }
    // No object closed: the tail was cut inside one. Read its keys anyway;
    // a `detail` cut away entirely leaves the point and code as the evidence.
    let start = text.find("\"failure_point\":")?;
    let cut = &text[start..];
    let point = field(cut, "failure_point")?;
    let code = Code::parse(field(cut, "error_code")?)?;
    let detail = match field(cut, "detail") {
        Some(detail) => detail,
        None => "",
    };
    named(point, code, detail)
}
