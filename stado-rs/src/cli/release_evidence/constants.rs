//! The words these two commands publish: the state of a log, the health of a
//! candidate, the phase of a rollout, the verdict over it, what blocks it and
//! the command that clears the block.

/// The log file was there and had bytes in it.
pub const STREAM_READ: &str = "read";
/// No such file on the host. Reported as its own word rather than as an
/// empty `lines` array: "the product wrote nothing" and "the product never
/// got far enough to have a log opened for it" send an operator to opposite
/// places, and the incident turned on exactly that distinction.
pub const STREAM_MISSING: &str = "missing";
/// The file exists and is zero bytes — the agent opened it, so the spawn
/// happened, and the product said nothing before it went.
pub const STREAM_EMPTY: &str = "empty";

/// The candidate answered its declared readiness path with a 2xx.
pub const HEALTH_OK: &str = "ok";
/// Nothing answered the candidate port at all.
pub const HEALTH_UNREACHABLE: &str = "unreachable";
/// The rollout state names no candidate, so there was nothing to probe.
/// Not [`HEALTH_UNREACHABLE`]: no candidate is a rollout that is not in
/// flight, and a dead candidate is a rollout that is failing.
pub const HEALTH_NO_CANDIDATE: &str = "no_candidate";
/// The target declares no readiness path (a `replace` strategy has nothing
/// HTTP to ask), so no probe was made.
pub const HEALTH_UNPROBED: &str = "unprobed";

/// No host has published a rollout status for this product and no state
/// file could be read, so the phase is unknown rather than idle.
pub const PHASE_UNREPORTED: &str = "unreported";

/// Observed release equals desired release and nothing is in flight.
pub const VERDICT_SETTLED: &str = "settled";
/// A candidate is staged or running, or observed still differs from
/// desired with nothing blocking the agent.
pub const VERDICT_ROLLING: &str = "rolling";
/// The rollout cannot proceed on its own. Both causes are silent today:
/// a quarantined desired digest is skipped forever, and an unresolved disk
/// gate stops the host from claiming anything at all.
pub const VERDICT_BLOCKED: &str = "blocked";

/// The desired artifact's digest is in the host's quarantine map. The agent
/// will refuse this exact release on every pass until the digest is cleared
/// (`stado release quarantine clear`) or a new version is promoted.
pub const BLOCKER_DESIRED_DIGEST_QUARANTINED: &str = "desired_digest_quarantined";
/// A candidate is recorded and is not answering its readiness path. Listed
/// as a blocker even while the verdict stays [`VERDICT_ROLLING`], because
/// the rollout is still inside its readiness window and the next thing that
/// happens to it is a quarantine.
pub const BLOCKER_CANDIDATE_NOT_READY: &str = "candidate_not_ready";
/// The last few quarantines on this host all failed for one named cause, so
/// the agent will refuse the next candidate rather than spend it on the same
/// wall. Reported here because the only other way to discover the refusal is
/// to promote a candidate and read it afterwards, which costs the candidate
/// this rule exists to save.
pub const BLOCKER_REPEATING_CAUSE: &str = "repeating_quarantine_cause";

/// The command that retires [`BLOCKER_DESIRED_DIGEST_QUARANTINED`]. Named here
/// rather than left to the operator, because a verdict that identifies a
/// permanent blocker and withholds the one command that clears it is half a
/// diagnosis.
pub const REMEDY_DESIRED_DIGEST_QUARANTINED: &str =
    "stado release quarantine clear --digest <digest> --reason <text>";
