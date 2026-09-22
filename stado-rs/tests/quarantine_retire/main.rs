//! A refusal that named the host must not outlive the host's condition.
//!
//! # The invariant
//!
//! `QuarantineCause::holds_the_candidate` divides refusals into statements
//! about the release and statements about the host. A host that could not
//! answer a three-second readiness probe said nothing about the bytes it
//! failed to start, so the agent retires that record by itself and rolls the
//! desired digest out again. A refusal that names the candidate — a vault that
//! will not open, an undeclared rollback compatibility — is still cleared only
//! by `stado release quarantine clear`, on the operator's audit line.
//!
//! # What went wrong
//!
//! On `lukasz-macbook` the desired Skarbiec digest
//! `55f2cf470e293d03c920ee1b4184e5144c98acbc7fe6315771be892b1b9791b4` was
//! quarantined at 2026-09-17T21:50:31Z with `active release lost readiness:
//! http://127.0.0.1:18788/readyz did not answer within 3s`. The agent's tick
//! read the map, set `phase: quarantined`, and returned — on that pass and
//! every pass after it. Three days later `release doctor` still reported
//! `observed -`, and every command resolving the release-controlled Skarbiec
//! binary refused with `no observed active release (phase Quarantined)`:
//! credential reads, grant reads, `release catalog declare-publisher`, and
//! with them the Most provider credential and the fleet's release publication.
//!
//! # What is defended here
//!
//! The persisted effects, not a log line: the record leaves the state
//! document, this agent's own retirement is appended to the same audit trail
//! the operator command writes, a second retirement inside the cooldown is
//! refused so a host that still cannot run the release does not spend one
//! candidate per tick, a retirement older than the cooldown is allowed again,
//! and a candidate-naming cause is left exactly as it was found.

mod fixture;

#[path = "cases/left_alone.rs"]
mod left_alone;
#[path = "cases/retiring.rs"]
mod retiring;
