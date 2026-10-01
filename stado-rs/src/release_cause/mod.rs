//! Why a release candidate was quarantined, as a name rather than a symptom.
//!
//! NO Python original. A product can go to zero processes with
//! `active_version: null` while `stado release doctor <product>` prints a long
//! list of quarantine rows. Every row records what the agent saw from outside
//! — the candidate did not answer `/readyz`, or its pid was gone — and buries
//! whatever the candidate said about itself in a truncated `stderr` dump at
//! the end of the same sentence.
//!
//! The cause is in the candidate's own log for the records that have one —
//! for example vault fields behind the provider credentials that cannot
//! serve, so Skarbiec refuses to issue capabilities, so the gateway obtains no
//! credential and can never become ready. Several rows can name that one path
//! in different words while more candidates burn saying nothing at all, and
//! nobody reading the table can see that several rows are one thing, because
//! the table has no word for the thing.
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
//! quarantine belongs to. Failure modes that no recorded quarantine exhibits
//! are deliberately absent — a name with no evidence behind it is a label
//! waiting to be applied wrongly.
//!
//! [`QuarantineCause::Unclassified`] is load-bearing and is not a defect.
//! Many live rows land in it: some say only `candidate did not become ready
//! before deadline`, from before the agent retained any of the candidate's
//! output at all; some retain a symptom and no log; and the rest retain a log
//! that names no failure — candidates that stop after `issuing runtime
//! capabilities` stay unclassified even when the whole file is read, because
//! the product wrote nothing to classify.
//!
//! Forcing those into the nearest-looking class would be the same mistake as
//! recording the symptom, with more confidence. They are reported as
//! unclassified, and the count of them is the honest measure of how much this
//! host's evidence is worth.

mod cause;
mod classify;
mod refusal;
mod tally;
mod wall;

pub use cause::QuarantineCause;
pub use classify::{classify, classify_observed, Classification};
pub use refusal::Refusal;
pub use tally::{dominant, tally};
pub use wall::{read_routes_verify, routes_verify_detail, CausePredicate, WallVerdict};
