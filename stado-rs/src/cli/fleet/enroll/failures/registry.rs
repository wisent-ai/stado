//! The causes an enrollment meets in the registry: reading it, the fleet's
//! enrollment catalog in it, the entry and fleet it names, and writing the new
//! entry back.

use wisent_errors::{Cause, Code};

use super::{Step, PLAIN};

pub(crate) static REGISTRY_MISSING: Cause = Cause {
    id: "registry-missing",
    code: Code::NotFound,
    failed: "reading the fleet registry",
    cause: "the store this machine is configured with holds no registry document",
    observed: "{answer}",
    remedy: "import one with `stado onboarding --import-registry FILE`, or point this machine at the store that holds it (`stado config show` names the store in use)",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_without_a_registry",
    ..PLAIN
};

pub(crate) static REGISTRY_UNREACHABLE: Cause = Cause {
    id: "registry-unreachable",
    code: Code::InfraDown,
    failed: "reading the fleet registry",
    cause: "the store holding the registry, or the registry authority this machine reads it through, did not answer",
    observed: "{answer}",
    remedy: "bring the store or the authority back (`stado doctor` checks storage and the registry in order), then run the same command again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_while_the_registry_store_is_down",
    ..PLAIN
};

pub(crate) static REGISTRY_TIMEOUT: Cause = Cause {
    id: "registry-timeout",
    code: Code::Timeout,
    failed: "reading the fleet registry",
    cause: "the store holding the registry did not answer within the wait's deadline",
    observed: "{answer}",
    remedy: "run the same command again; when it keeps timing out, `stado waits measured` shows how long registry reads take here and `stado doctor` which check is slow",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_when_the_registry_read_times_out",
    ..PLAIN
};

pub(crate) static REGISTRY_AUTH_REFUSED: Cause = Cause {
    id: "registry-auth-refused",
    code: Code::Auth,
    failed: "reading the fleet registry",
    cause: "the store refused the credential this machine reads the registry with",
    observed: "{answer}",
    remedy: "renew or re-grant this machine's storage credential (`stado doctor` names the store and the credential it refused), then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_when_the_store_refuses_its_credential",
    ..PLAIN
};

pub(crate) static REGISTRY_INVALID: Cause = Cause {
    id: "registry-invalid",
    code: Code::Config,
    failed: "reading the fleet registry",
    cause: "what the store holds is not a registry this Stado reads: not JSON, not an object, or written under a newer schema",
    observed: "{answer}",
    remedy: "read it with `stado registry pull` and check a copy with `stado registry validate FILE`; a document from a newer Stado needs this machine's Stado updated (`stado product update stado`)",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_registry_that_is_not_json",
    ..PLAIN
};

pub(crate) static REGISTRY_STORE_FAILED: Cause = Cause {
    id: "registry-store-failed",
    code: Code::InfraDown,
    failed: "reading the fleet registry",
    cause: "the store answered the read with an error of its own that Stado does not classify further: an I/O error, a full or unreadable disk, a backend protocol error",
    observed: "{answer}",
    remedy: "fix what the store's answer names (for a local store: the disk and the permissions of its directory), then run again; `stado doctor` checks the storage backend",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_an_unreadable_local_store",
    ..PLAIN
};

/// Reading the registry, by the class the store's answer stated.
pub(crate) static REGISTRY_READ: Step = Step {
    missing: &REGISTRY_MISSING,
    unreachable: &REGISTRY_UNREACHABLE,
    timeout: &REGISTRY_TIMEOUT,
    auth: &REGISTRY_AUTH_REFUSED,
    refused: &REGISTRY_INVALID,
    invalid: &REGISTRY_INVALID,
    failed: &REGISTRY_STORE_FAILED,
};

pub(crate) static CATALOG_INVALID: Cause = Cause {
    id: "enrollment-catalog-invalid",
    code: Code::Config,
    failed: "reading the fleet's enrollment catalog (`registry.enrollment`)",
    cause: "the catalog is not in the shape Stado reads",
    observed: "{answer}",
    remedy: "correct it with `stado registry set --path enrollment.<field> --value true` (or false); `stado fleet catalog` prints what is declared",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_malformed_enrollment_catalog",
    ..PLAIN
};

pub(crate) static ENROLL_DISABLED: Cause = Cause {
    id: "enroll-disabled",
    code: Code::Refused,
    failed: "enrolling '{name}' from the control plane",
    cause: "the fleet's catalog switches control-plane enrollment off: registry.enrollment.allow_enroll is false",
    observed: "methods this fleet allows: {allowed}",
    remedy: "add the machine by an allowed method (`stado fleet methods` prints each with its command), or switch this one on with `stado registry set --path enrollment.allow_enroll --value true`",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_when_the_catalog_disables_it",
    ..PLAIN
};

pub(crate) static ADOPT_DISABLED: Cause = Cause {
    id: "adopt-disabled",
    code: Code::Refused,
    failed: "installing the fleet's key on {ssh} (--install-key)",
    cause: "the fleet's catalog switches adoption off: registry.enrollment.allow_adopt is false",
    observed: "methods this fleet allows: {allowed}",
    remedy: "run without --install-key on a machine that already holds the fleet's key, use an allowed method (`stado fleet methods`), or switch adoption on with `stado registry set --path enrollment.allow_adopt --value true`",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_install_key_when_adoption_is_disabled",
    ..PLAIN
};

pub(crate) static JOIN_DISABLED: Cause = Cause {
    id: "join-disabled",
    code: Code::Refused,
    failed: "machine-initiated enrollment of '{name}'",
    cause: "the fleet's catalog switches it off: registry.enrollment.allow_join is false",
    observed: "methods this fleet allows: {allowed}",
    remedy: "add the machine by an allowed method (`stado fleet methods`), or switch joining on with `stado registry set --path enrollment.allow_join --value true`",
    test: "tests/fleet/join_failures.rs::join_refuses_when_the_catalog_disables_it",
    ..PLAIN
};

pub(crate) static INVITE_DISABLED: Cause = Cause {
    id: "invite-disabled",
    code: Code::Refused,
    failed: "approving the invited machine '{name}'",
    cause: "the fleet's catalog switches invitations off: registry.enrollment.allow_invite is false",
    observed: "methods this fleet allows: {allowed}",
    remedy: "switch invitations on with `stado registry set --path enrollment.allow_invite --value true`, or drop the request with `stado fleet reject {hostname}` and add the machine another way",
    test: "tests/fleet/approve_failures.rs::approve_refuses_an_invited_request_when_invitations_are_disabled",
    ..PLAIN
};

pub(crate) static TARGETS_MALFORMED: Cause = Cause {
    id: "registry-targets-malformed",
    code: Code::Config,
    failed: "looking '{name}' up among the registry's targets",
    cause: "registry.targets is not an array",
    observed: "registry.targets holds {targets}",
    remedy: "repair the document: `stado registry pull > registry.json`, make `targets` a list, `stado registry validate registry.json`, then `stado registry push`",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_registry_whose_targets_is_not_a_list",
    ..PLAIN
};

pub(crate) static ALREADY_REGISTERED: Cause = Cause {
    id: "already-registered",
    code: Code::Refused,
    failed: "registering '{name}'",
    cause: "a target named '{name}' is already registered",
    observed: "its entry: {entry}",
    remedy: "register this machine under another name, or retire the existing entry first with `stado registry host remove {name}` if it is the same machine set up again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_name_already_registered",
    ..PLAIN
};

pub(crate) static ALREADY_HAS_BEACON: Cause = Cause {
    id: "already-has-beacon",
    code: Code::Refused,
    failed: "repairing the legacy entry '{name}' by enrolling it",
    cause: "'{name}' is registered without a channel but publishes a health beacon, so it is a live machine, not a legacy declaration enrollment may replace",
    observed: "a health beacon is published for '{name}'",
    remedy: "give the existing entry its channel with `stado registry host edit {name}`, or enroll this machine under another name",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_to_replace_a_target_that_publishes_a_beacon",
    ..PLAIN
};

pub(crate) static BEACON_UNREADABLE: Cause = Cause {
    id: "beacon-unreadable",
    code: Code::InfraDown,
    failed: "proving that the legacy entry '{name}' publishes no health beacon",
    cause: "the store holding health beacons could not be read, so replacing the entry could erase a live machine",
    observed: "{answer}",
    remedy: "bring the store back (`stado doctor` checks storage), then run the same command again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_takeover_while_beacons_are_unreadable",
    ..PLAIN
};

pub(crate) static FLEET_UNDECLARED: Cause = Cause {
    id: "fleet-undeclared",
    code: Code::Refused,
    failed: "placing '{name}' in fleet '{fleet}'",
    cause: "the registry declares no fleet named '{fleet}'",
    observed: "declared fleets: {declared}",
    remedy: "declare it with `stado fleet create {fleet}`, or name one of the declared fleets with --fleet",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_an_undeclared_fleet",
    ..PLAIN
};

pub(crate) static FLEETS_MALFORMED: Cause = Cause {
    id: "fleets-malformed",
    code: Code::Config,
    failed: "reading the fleets the registry declares",
    cause: "registry.fleets is not in the shape Stado reads",
    observed: "{answer}",
    remedy: "correct `fleets` (a list of objects with a `name`): `stado registry pull --path fleets` shows it, `stado registry set --path fleets --value '[...]'` writes it",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_malformed_fleet_list",
    ..PLAIN
};

pub(crate) static REGISTRY_MOVED: Cause = Cause {
    id: "registry-moved",
    code: Code::Refused,
    exit: Some(crate::cli::registry::REGISTRY_CONFLICT_EXIT),
    failed: "writing '{name}' into the registry",
    cause: "the registry changed after this command read it, and every check, key install and probe of this run was decided against the document it read",
    observed: "{answer}",
    remedy: "run the same command again: it reads the registry as it is now and decides again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_when_the_registry_moves_during_the_run",
    ..PLAIN
};

pub(crate) static REGISTRY_WRITE_REFUSED: Cause = Cause {
    id: "registry-write-refused",
    code: Code::Refused,
    failed: "writing '{name}' into the registry",
    cause: "the registry contract refused the new document: another target already declares the hostname, or the document would not validate",
    observed: "{answer}",
    remedy: "resolve what the contract names; `stado registry host show HOST` shows the entry it collides with",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_hostname_another_target_declares",
    ..PLAIN
};

pub(crate) static REGISTRY_WRITE_UNREACHABLE: Cause = Cause {
    id: "registry-write-unreachable",
    code: Code::InfraDown,
    failed: "writing '{name}' into the registry",
    cause: "the store did not answer the write, or reading the write back did not find it",
    observed: "{answer}",
    remedy: "bring the store back (`stado doctor`), check with `stado registry host show {name}` whether the entry landed, and run the same command again if it did not",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_while_the_store_refuses_writes",
    ..PLAIN
};

pub(crate) static REGISTRY_WRITE_TIMEOUT: Cause = Cause {
    id: "registry-write-timeout",
    code: Code::Timeout,
    failed: "writing '{name}' into the registry",
    cause: "the store did not answer the write within the wait's deadline, so whether it landed is not known",
    observed: "{answer}",
    remedy: "check with `stado registry host show {name}` whether the entry landed, and run the same command again if it did not",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_when_the_registry_write_times_out",
    ..PLAIN
};

pub(crate) static REGISTRY_WRITE_AUTH_REFUSED: Cause = Cause {
    id: "registry-write-auth-refused",
    code: Code::Auth,
    failed: "writing '{name}' into the registry",
    cause: "the store refused this machine's credential for writing the registry",
    observed: "{answer}",
    remedy: "write from a machine whose storage credential may write the registry, or grant this one (`stado doctor` names the store and the credential it refused)",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_when_the_store_refuses_the_write_credential",
    ..PLAIN
};

pub(crate) static REGISTRY_WRITE_FAILED: Cause = Cause {
    id: "registry-write-failed",
    code: Code::InfraDown,
    failed: "writing '{name}' into the registry",
    cause: "the store answered the write with an error of its own that Stado does not classify further: an I/O error, a full disk, a missing object, a backend protocol error",
    observed: "{answer}",
    remedy: "fix what the store's answer names, check with `stado registry host show {name}` whether the entry landed, and run again if it did not",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_when_the_local_store_cannot_be_written",
    ..PLAIN
};

/// Writing the registry, by the class the store's answer stated. A lost
/// compare-and-swap is told apart by its own exit status before this step
/// classifies anything ([`super::write_failure`]).
pub(crate) static REGISTRY_WRITE: Step = Step {
    missing: &REGISTRY_WRITE_FAILED,
    unreachable: &REGISTRY_WRITE_UNREACHABLE,
    timeout: &REGISTRY_WRITE_TIMEOUT,
    auth: &REGISTRY_WRITE_AUTH_REFUSED,
    refused: &REGISTRY_WRITE_REFUSED,
    invalid: &REGISTRY_WRITE_REFUSED,
    failed: &REGISTRY_WRITE_FAILED,
};
