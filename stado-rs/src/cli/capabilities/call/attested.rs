//! The attestation alternative of `stado capabilities call`: a request is
//! answered by the statement recorded for it, kept for one, or, with
//! `--await-statement`, waited on until one is recorded.

use serde_json::Value;

use super::Outcome;
use crate::capabilities::alternatives::Family;

/// Answer the request from the statement recorded for it, or keep it. With
/// `wait`, a kept request is waited on until a statement answers it
/// (`attestation::await_statement`), so a caller with no other alternative
/// is answered in this call instead of refused and asked to repeat it.
pub(super) fn attested(family: &Family, consumer: &str, request: &Value, wait: bool) -> Outcome {
    if wait {
        return match super::super::attestation::await_statement(family, consumer, request) {
            Ok((_, answer)) => Outcome::Answered(answer),
            Err(cause) => Outcome::Passed(cause),
        };
    }
    match super::super::attestation::answer_or_keep(family, consumer, request) {
        Ok((_, Some(answer))) => Outcome::Answered(answer),
        Ok((digest, None)) => Outcome::Passed(format!(
            "no statement answers request {digest} yet; read it with `stado capabilities attestation show {} --consumer {consumer} {digest}` and answer it with `stado capabilities attestation record {} --consumer {consumer} {digest} --checked <what was checked>`, the answer on standard input; a caller that passes --await-statement is answered when it is recorded",
            family.id, family.id
        )),
        Err(cause) => Outcome::Passed(cause),
    }
}
