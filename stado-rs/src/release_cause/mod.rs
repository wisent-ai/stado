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

mod cause;
mod classify;
mod tally;
mod wall;

pub use cause::QuarantineCause;
pub use classify::{classify, Classification};
pub use tally::{dominant, tally};
pub use wall::{read_routes_verify, routes_verify_detail, CausePredicate, WallVerdict};
