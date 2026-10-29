//! The causes of the machine-initiated path: the store the join requests live
//! in (`enrollments/`), the request a machine filed, and what approving one
//! leaves behind when a later step fails.

use wisent_errors::{Cause, Code};

use super::{Step, PLAIN};

pub(crate) static STORE_MISSING: Cause = Cause {
    id: "store-missing",
    code: Code::NotFound,
    failed: "reaching the enrollment requests in the store (`enrollments/`)",
    cause: "the store this machine is configured with does not exist",
    observed: "{answer}",
    remedy: "point this machine at the store the fleet uses (`stado config show` names the one in use), then run again",
    test: "tests/fleet/join_failures.rs::join_refuses_a_store_that_does_not_exist",
    ..PLAIN
};

pub(crate) static STORE_UNREACHABLE: Cause = Cause {
    id: "store-unreachable",
    code: Code::InfraDown,
    failed: "reaching the enrollment requests in the store (`enrollments/`)",
    cause: "the store did not answer",
    observed: "{answer}",
    remedy: "bring the store back (`stado doctor` checks storage), then run the same command again",
    test: "tests/fleet/join_failures.rs::join_refuses_while_the_store_is_down",
    ..PLAIN
};

pub(crate) static STORE_TIMEOUT: Cause = Cause {
    id: "store-timeout",
    code: Code::Timeout,
    failed: "reaching the enrollment requests in the store (`enrollments/`)",
    cause: "the store did not answer within the wait's deadline",
    observed: "{answer}",
    remedy: "run the same command again; when it keeps timing out, `stado doctor` shows which storage check is slow",
    test: "tests/fleet/join_failures.rs::join_refuses_when_the_store_times_out",
    ..PLAIN
};

pub(crate) static STORE_AUTH_REFUSED: Cause = Cause {
    id: "store-auth-refused",
    code: Code::Auth,
    failed: "reaching the enrollment requests in the store (`enrollments/`)",
    cause: "the store refused this machine's storage credential",
    observed: "{answer}",
    remedy: "give this machine a storage credential the fleet's store accepts (`stado doctor` names the store and the credential it refused), then run again",
    test: "tests/fleet/join_failures.rs::join_refuses_when_the_store_refuses_its_credential",
    ..PLAIN
};

pub(crate) static STORE_UNCONFIGURED: Cause = Cause {
    id: "store-unconfigured",
    code: Code::Config,
    failed: "reaching the enrollment requests in the store (`enrollments/`)",
    cause: "this machine's storage configuration is incomplete or names no usable store",
    observed: "{answer}",
    remedy: "complete it (`stado config show` prints it, `stado doctor` names the missing setting), then run again",
    test: "tests/fleet/join_failures.rs::join_refuses_an_incomplete_storage_configuration",
    ..PLAIN
};

pub(crate) static STORE_FAILED: Cause = Cause {
    id: "store-failed",
    code: Code::InfraDown,
    failed: "reaching the enrollment requests in the store (`enrollments/`)",
    cause: "the store answered with an error of its own that Stado does not classify further: an I/O error, a full or unwritable disk, a backend protocol error",
    observed: "{answer}",
    remedy: "fix what the store's answer names (for a local store: the disk and the permissions of its directory), then run again",
    test: "tests/fleet/join_failures.rs::join_refuses_an_unwritable_local_store",
    ..PLAIN
};

/// The request store, by the class its answer stated.
pub(crate) static REQUEST_STORE: Step = Step {
    missing: &STORE_MISSING,
    unreachable: &STORE_UNREACHABLE,
    timeout: &STORE_TIMEOUT,
    auth: &STORE_AUTH_REFUSED,
    refused: &STORE_UNCONFIGURED,
    invalid: &STORE_UNCONFIGURED,
    failed: &STORE_FAILED,
};

pub(crate) static REQUEST_MISSING: Cause = Cause {
    id: "request-missing",
    code: Code::NotFound,
    failed: "approving '{hostname}'",
    cause: "the store holds no join request for '{hostname}'",
    observed: "pending requests: {pending}",
    remedy: "run `stado fleet join` on the machine first, or approve one of the pending names (`stado fleet pending` lists them)",
    test: "tests/fleet/approve_failures.rs::approve_refuses_a_hostname_without_a_request",
    ..PLAIN
};

pub(crate) static REQUEST_UNREADABLE: Cause = Cause {
    id: "request-unreadable",
    code: Code::Config,
    failed: "approving '{hostname}'",
    cause: "the stored request enrollments/{hostname}.json is not JSON",
    observed: "{answer}",
    remedy: "drop it with `stado fleet reject {hostname}` and run `stado fleet join` on the machine again",
    test: "tests/fleet/approve_failures.rs::approve_refuses_a_request_that_is_not_json",
    ..PLAIN
};

pub(crate) static REQUEST_DECIDED: Cause = Cause {
    id: "request-decided",
    code: Code::Refused,
    failed: "approving '{hostname}'",
    cause: "the request for '{hostname}' is no longer pending: it records status '{status}'",
    observed: "enrollments/{hostname}.json status: {status}",
    remedy: "nothing is left to approve; a machine that must join again runs `stado fleet join` after `stado fleet reject {hostname}` removes the old request",
    test: "tests/fleet/approve_failures.rs::approve_refuses_a_request_already_approved",
    ..PLAIN
};

pub(crate) static REQUEST_INCOMPLETE: Cause = Cause {
    id: "request-incomplete",
    code: Code::Refused,
    failed: "approving '{hostname}'",
    cause: "the request for '{hostname}' does not state its {field}",
    observed: "enrollments/{hostname}.json: {request}",
    remedy: "drop it with `stado fleet reject {hostname}`; `stado fleet join` on the machine files a complete one",
    test: "tests/fleet/approve_failures.rs::approve_refuses_a_request_without_an_operating_system",
    ..PLAIN
};

pub(crate) static REQUEST_STATUS_UNWRITTEN: Cause = Cause {
    id: "request-status-unwritten",
    code: Code::InfraDown,
    failed: "marking the join request for '{hostname}' approved",
    cause: "'{name}' is registered, but its request could not be marked approved, so `stado fleet pending` still lists it",
    observed: "{answer}",
    remedy: "drop the stale request with `stado fleet reject {hostname}`; the registration stands",
    test: "tests/fleet/approve_failures.rs::approve_reports_a_request_it_could_not_mark",
    ..PLAIN
};

pub(crate) static INVITE_UNSPENT: Cause = Cause {
    id: "invite-unspent",
    code: Code::InfraDown,
    failed: "spending invite {invite}",
    cause: "'{name}' is registered and its request approved, but the invite that paid for it could not be marked spent, so it may be redeemed again",
    observed: "{answer}",
    remedy: "revoke it with `stado fleet invite revoke {invite}`; the registration stands",
    test: "tests/fleet/approve_failures.rs::approve_reports_an_invite_it_could_not_spend",
    ..PLAIN
};
