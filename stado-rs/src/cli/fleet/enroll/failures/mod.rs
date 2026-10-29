//! How adding a machine to the fleet fails: every cause `stado fleet enroll`,
//! `stado fleet join` and `stado fleet approve` can meet, declared once.
//!
//! A cause is raised where its step breaks, with the values that make its
//! sentence concrete; the same declaration is what `stado errors show
//! fleet.enroll` prints and what the documentation page lists, so the three
//! cannot disagree. A cause a step below reports in its own words is not
//! re-worded: it is classified by the class that step stated ([`Step`]) and
//! quoted as the observed state.

use wisent_errors::{Cause, Code, Operation};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

mod channel;
mod machine;
mod registry;
mod requests;

pub(crate) use channel::*;
pub(crate) use machine::*;
pub(crate) use registry::*;
pub(crate) use requests::*;

/// The fields every cause shares unless it says otherwise: not a usage
/// refusal, and the exit status its code derives.
pub(crate) const PLAIN: Cause = Cause {
    id: "",
    code: Code::Refused,
    usage: false,
    exit: None,
    failed: "",
    cause: "",
    observed: "",
    remedy: "",
    test: "",
};

/// A refusal of the request's own form, exiting with the usage status.
pub(crate) const USAGE: Cause = Cause {
    usage: true,
    ..PLAIN
};

/// One step whose failure is reported by the layer below it: the declared
/// cause for each class that layer can state.
pub(crate) struct Step {
    pub(crate) missing: &'static Cause,
    pub(crate) unreachable: &'static Cause,
    pub(crate) timeout: &'static Cause,
    pub(crate) auth: &'static Cause,
    pub(crate) refused: &'static Cause,
    pub(crate) invalid: &'static Cause,
    /// What the layer below stated no class for, or `unknown`.
    pub(crate) failed: &'static Cause,
}

impl Step {
    /// The declared cause of `error`, by the class it stated.
    fn cause_of(&self, error: &CmdError) -> &'static Cause {
        match error.failure {
            Some(FailureCode::NotFound) => self.missing,
            Some(FailureCode::InfraDown | FailureCode::RateLimit) => self.unreachable,
            Some(FailureCode::Timeout) => self.timeout,
            Some(FailureCode::Auth) => self.auth,
            Some(FailureCode::Refused) => self.refused,
            Some(FailureCode::Config) => self.invalid,
            Some(FailureCode::Unknown) | None => self.failed,
        }
    }

    /// `error`, raised as this step's declared cause with its own sentence as
    /// the observed state. One that is already a declared cause is passed on
    /// unchanged: the step below already said which cause it is.
    pub(crate) fn raise(&self, error: CmdError, values: &[(&str, &str)]) -> CmdError {
        if error.declared.is_some() {
            return error;
        }
        let answer = error.to_string();
        let mut filled = values.to_vec();
        filled.push(("answer", &answer));
        CmdError::declared(self.cause_of(&error), &filled)
    }
}

/// The part of a registered entry a refusal shows: the channel and the
/// hostnames that tell whether it is the same machine. The whole entry is
/// `stado registry host show NAME`.
pub(crate) fn entry_summary(entry: &serde_json::Value) -> String {
    format!(
        "ssh {}, hostnames {}, kind {}",
        entry["ssh"], entry["hostnames"], entry["kind"]
    )
}

/// A failed registry write: a cause the transform already declared, a lost
/// compare-and-swap told apart by its own exit status, or the store's answer
/// by its class.
pub(crate) fn write_failure(error: CmdError, values: &[(&str, &str)]) -> CmdError {
    if error.declared.is_some() {
        return error;
    }
    if error.code == crate::cli::registry::REGISTRY_CONFLICT_EXIT {
        let answer = error.to_string();
        let mut filled = values.to_vec();
        filled.push(("answer", &answer));
        return CmdError::declared(&REGISTRY_MOVED, &filled);
    }
    REGISTRY_WRITE.raise(error, values)
}

/// `stado fleet enroll NAME --ssh DEST`.
pub static ENROLL: Operation = Operation {
    id: "fleet.enroll",
    command: "stado fleet enroll",
    summary: "Register a machine verified over SSH: its identity is probed before anything is written, it is placed in a fleet with --fleet, and with --bootstrap Stado is installed there and the registration rolled back when the install fails.",
    docs: "https://stado.wisent.com/docs/cli/fleet/enroll",
    causes: &[
        &ARGUMENTS,
        &NAME_INVALID,
        &DESTINATION_INVALID,
        &KIND_UNKNOWN,
        &REGISTRY_MISSING,
        &REGISTRY_UNREACHABLE,
        &REGISTRY_TIMEOUT,
        &REGISTRY_AUTH_REFUSED,
        &REGISTRY_INVALID,
        &REGISTRY_STORE_FAILED,
        &CATALOG_INVALID,
        &ENROLL_DISABLED,
        &ADOPT_DISABLED,
        &TARGETS_MALFORMED,
        &ALREADY_REGISTERED,
        &ALREADY_HAS_BEACON,
        &BEACON_UNREADABLE,
        &FLEET_UNDECLARED,
        &FLEETS_MALFORMED,
        &ADOPT_KEY_MISSING,
        &ADOPT_UNREACHABLE,
        &ADOPT_TIMEOUT,
        &ADOPT_REFUSED,
        &ADOPT_WRITE_REFUSED,
        &ADOPT_SETTING_INVALID,
        &ADOPT_FAILED,
        &CHANNEL_KEY_MISSING,
        &KEY_STORE_UNREACHABLE,
        &KEY_STORE_TIMEOUT,
        &KEY_STORE_REFUSED,
        &KEY_FILE_REFUSED,
        &KEY_SETTING_INVALID,
        &KEY_FILE_UNWRITABLE,
        &SSH_UNSTARTABLE,
        &DESTINATION_UNRESOLVABLE,
        &CONNECTION_REFUSED,
        &DESTINATION_TIMEOUT,
        &NO_ROUTE,
        &SSH_AUTH_REFUSED,
        &HOST_KEY_CHANGED,
        &PROBE_COMMAND_FAILED,
        &CHANNEL_FAILED,
        &IDENTITY_AMBIGUOUS,
        &PLATFORM_UNSUPPORTED,
        &PLATFORM_MISMATCH,
        &PLATFORM_DECLARATION_MALFORMED,
        &REGISTRY_MOVED,
        &REGISTRY_WRITE_REFUSED,
        &REGISTRY_WRITE_UNREACHABLE,
        &REGISTRY_WRITE_TIMEOUT,
        &REGISTRY_WRITE_AUTH_REFUSED,
        &REGISTRY_WRITE_FAILED,
        &BOOTSTRAP_FAILED,
        &BOOTSTRAP_UNREACHABLE,
        &ROLLBACK_FAILED,
        &NETWORK_JOIN_FAILED,
        &NETWORK_RECEIPT_UNREADABLE,
    ],
};

/// `stado fleet join`, on the machine being added.
pub static JOIN: Operation = Operation {
    id: "fleet.join",
    command: "stado fleet join",
    summary: "Announce this machine to the fleet: its hostname, operating system and architecture are filed as a join request in the store for the operator to approve. A registry this machine cannot read is said and leaves the catalog's gate to approve.",
    docs: "https://stado.wisent.com/docs/cli/fleet/join",
    causes: &[
        &ARGUMENTS,
        &CATALOG_INVALID,
        &JOIN_DISABLED,
        &PLATFORM_UNSUPPORTED,
        &STORE_MISSING,
        &STORE_UNREACHABLE,
        &STORE_TIMEOUT,
        &STORE_AUTH_REFUSED,
        &STORE_UNCONFIGURED,
        &STORE_FAILED,
    ],
};

/// `stado fleet approve HOSTNAME`, on the control plane.
pub static APPROVE: Operation = Operation {
    id: "fleet.approve",
    command: "stado fleet approve",
    summary: "Turn a pending join request into a registered target. A request an invite filed carries a channel and is enrolled exactly as `stado fleet enroll --bootstrap` enrolls; one without a channel is registered from the machine's own report.",
    docs: "https://stado.wisent.com/docs/cli/fleet/approve",
    causes: &[
        &ARGUMENTS,
        &NAME_INVALID,
        &STORE_MISSING,
        &STORE_UNREACHABLE,
        &STORE_TIMEOUT,
        &STORE_AUTH_REFUSED,
        &STORE_UNCONFIGURED,
        &STORE_FAILED,
        &REQUEST_MISSING,
        &REQUEST_UNREADABLE,
        &REQUEST_DECIDED,
        &REQUEST_INCOMPLETE,
        &REGISTRY_MISSING,
        &REGISTRY_UNREACHABLE,
        &REGISTRY_TIMEOUT,
        &REGISTRY_AUTH_REFUSED,
        &REGISTRY_INVALID,
        &REGISTRY_STORE_FAILED,
        &CATALOG_INVALID,
        &JOIN_DISABLED,
        &INVITE_DISABLED,
        &ENROLL_DISABLED,
        &PLATFORM_UNSUPPORTED,
        &TARGETS_MALFORMED,
        &ALREADY_REGISTERED,
        &ALREADY_HAS_BEACON,
        &BEACON_UNREADABLE,
        &FLEET_UNDECLARED,
        &FLEETS_MALFORMED,
        &DESTINATION_INVALID,
        &KIND_UNKNOWN,
        &CHANNEL_KEY_MISSING,
        &KEY_STORE_UNREACHABLE,
        &KEY_STORE_TIMEOUT,
        &KEY_STORE_REFUSED,
        &KEY_FILE_REFUSED,
        &KEY_SETTING_INVALID,
        &KEY_FILE_UNWRITABLE,
        &SSH_UNSTARTABLE,
        &DESTINATION_UNRESOLVABLE,
        &CONNECTION_REFUSED,
        &DESTINATION_TIMEOUT,
        &NO_ROUTE,
        &SSH_AUTH_REFUSED,
        &HOST_KEY_CHANGED,
        &PROBE_COMMAND_FAILED,
        &CHANNEL_FAILED,
        &IDENTITY_AMBIGUOUS,
        &PLATFORM_MISMATCH,
        &PLATFORM_DECLARATION_MALFORMED,
        &REGISTRY_MOVED,
        &REGISTRY_WRITE_REFUSED,
        &REGISTRY_WRITE_UNREACHABLE,
        &REGISTRY_WRITE_TIMEOUT,
        &REGISTRY_WRITE_AUTH_REFUSED,
        &REGISTRY_WRITE_FAILED,
        &BOOTSTRAP_FAILED,
        &BOOTSTRAP_UNREACHABLE,
        &ROLLBACK_FAILED,
        &NETWORK_JOIN_FAILED,
        &NETWORK_RECEIPT_UNREADABLE,
        &REQUEST_STATUS_UNWRITTEN,
        &INVITE_UNSPENT,
    ],
};

/// The fleet's enrollment operations, as `stado errors` lists them.
pub static OPERATIONS: &[&Operation] = &[&ENROLL, &JOIN, &APPROVE];
