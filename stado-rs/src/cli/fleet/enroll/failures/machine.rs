//! The causes that come from the request's own form and from the machine
//! itself: the command line, the name and destination it gives, what the
//! machine says it is, and the install and network join that follow the
//! registration.

use wisent_errors::{Cause, Code};

use super::{PLAIN, USAGE};

pub(crate) static ARGUMENTS: Cause = Cause {
    id: "arguments-refused",
    code: Code::Refused,
    failed: "reading the command line",
    cause: "an argument the command requires is missing, or one it does not take was given",
    observed: "the argument parser names the argument it refused and prints the usage line",
    remedy: "the command's `--help` lists every argument it takes and which are required",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_command_line_without_name_or_ssh",
    ..USAGE
};

pub(crate) static NAME_INVALID: Cause = Cause {
    id: "name-invalid",
    code: Code::Refused,
    failed: "enrolling '{name}'",
    cause: "'{name}' is not a lowercase target identifier: lowercase letters and digits, with '.', '_' or '-' only between them",
    observed: "the name given: '{name}'",
    remedy: "choose a name of that shape, such as '{suggestion}'",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_name_that_is_not_a_target_identifier",
    ..USAGE
};

pub(crate) static DESTINATION_INVALID: Cause = Cause {
    id: "destination-invalid",
    code: Code::Refused,
    failed: "verifying '{name}' over --ssh '{ssh}'",
    cause: "the SSH destination is empty, holds whitespace, or starts with '-', which ssh would read as an option",
    observed: "--ssh '{ssh}'",
    remedy: "give the machine as user@host or host, for example --ssh admin@{name}.local",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_destination_ssh_would_read_as_an_option",
    ..USAGE
};

pub(crate) static KIND_UNKNOWN: Cause = Cause {
    id: "kind-unknown",
    code: Code::Refused,
    failed: "enrolling '{name}' as kind '{kind}'",
    cause: "'{kind}' is not a host target kind this Stado declares",
    observed: "kinds this Stado declares: {kinds}",
    remedy: "pass one of them with --kind, or leave --kind out for 'local'",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_an_undeclared_kind",
    ..USAGE
};

pub(crate) static IDENTITY_AMBIGUOUS: Cause = Cause {
    id: "identity-ambiguous",
    code: Code::Refused,
    failed: "reading `{command}` on {ssh}",
    cause: "the machine answered with no line, or with more than one, where the probe reads exactly one",
    observed: "the answer: {answer}",
    remedy: "make that account's login shell print nothing besides the command's own output (a banner or a profile writing to standard output breaks the probe), then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_machine_whose_shell_prints_a_banner",
    ..PLAIN
};

pub(crate) static PLATFORM_UNSUPPORTED: Cause = Cause {
    id: "platform-unsupported",
    code: Code::Refused,
    failed: "enrolling '{name}'",
    cause: "the machine runs os={os}, arch={arch}, a platform Stado publishes no release for",
    observed: "release platforms: darwin-arm64 (Darwin arm64), linux-amd64 (Linux x86_64)",
    remedy: "add a machine on one of those platforms; this one cannot run a released Stado",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_platform_without_a_release",
    ..PLAIN
};

pub(crate) static PLATFORM_MISMATCH: Cause = Cause {
    id: "platform-mismatch",
    code: Code::Refused,
    failed: "repairing the legacy entry '{name}' by enrolling it",
    cause: "the entry declares release_platform {declared}, but the machine at {ssh} runs {platform}",
    observed: "declared {declared}, observed {platform}",
    remedy: "point --ssh at the machine the entry describes, or, when the machine was replaced, correct the entry with `stado registry set --path targets.{name}.release_platform --value {platform}`",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_to_take_over_an_entry_declaring_another_platform",
    ..PLAIN
};

pub(crate) static PLATFORM_DECLARATION_MALFORMED: Cause = Cause {
    id: "platform-declaration-malformed",
    code: Code::Config,
    failed: "repairing the legacy entry '{name}' by enrolling it",
    cause: "the entry's release_platform is not a platform name",
    observed: "release_platform holds {declared}",
    remedy: "set it to the platform's name with `stado registry set --path targets.{name}.release_platform --value {platform}`, then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_an_entry_whose_platform_is_not_text",
    ..PLAIN
};

pub(crate) static BOOTSTRAP_FAILED: Cause = Cause {
    id: "bootstrap-failed",
    code: Code::Refused,
    failed: "installing Stado on '{name}' (--bootstrap)",
    cause: "the install refused or failed for a reason retrying unchanged does not clear; the registration was rolled back, so the registry is as it was",
    observed: "{answer}",
    remedy: "fix what the install names on the machine, then run the same `stado fleet enroll` again",
    test: "tests/fleet/enroll_failures.rs::enroll_rolls_back_when_the_install_is_refused",
    ..PLAIN
};

pub(crate) static BOOTSTRAP_UNREACHABLE: Cause = Cause {
    id: "bootstrap-unreachable",
    code: Code::InfraDown,
    failed: "installing Stado on '{name}' (--bootstrap)",
    cause: "something the install depends on did not answer (the machine, the release store, a download); the registration was rolled back, so the registry is as it was",
    observed: "{answer}",
    remedy: "run the same `stado fleet enroll` again once what the answer names is back",
    test: "tests/fleet/enroll_failures.rs::enroll_rolls_back_when_the_install_cannot_reach_the_release",
    ..PLAIN
};

pub(crate) static ROLLBACK_FAILED: Cause = Cause {
    id: "rollback-failed",
    code: Code::InfraDown,
    failed: "installing Stado on '{name}', then removing its registration again",
    cause: "the install failed and the removal of the new entry failed too: '{name}' stays registered without a Stado installed",
    observed: "install: {answer}; removal: {rollback}",
    remedy: "remove the entry with `stado registry host remove {name}`, or finish the install with `stado bootstrap --target {name}`",
    test: "tests/fleet/enroll_failures.rs::enroll_reports_a_registration_it_could_not_roll_back",
    ..PLAIN
};

pub(crate) static NETWORK_JOIN_FAILED: Cause = Cause {
    id: "network-join-failed",
    code: Code::InfraDown,
    failed: "joining '{name}' to Stado's network",
    cause: "'{name}' is registered and has Stado installed, but `stado network up` on it did not finish, so the network is not its preferred path yet",
    observed: "{answer}",
    remedy: "run `stado network up` on '{name}' (over the channel: `ssh {ssh} .stado/bin/stado network up`); the registration and the install stand",
    test: "tests/fleet/enroll_failures.rs::enroll_reports_a_machine_registered_but_off_the_network",
    ..PLAIN
};

pub(crate) static NETWORK_RECEIPT_UNREADABLE: Cause = Cause {
    id: "network-receipt-unreadable",
    code: Code::Config,
    failed: "joining '{name}' to Stado's network",
    cause: "`stado network up` on '{name}' answered with something other than its JSON receipt: its Stado is another version than this one",
    observed: "{answer}",
    remedy: "bring '{name}' to this Stado's version with `stado host upgrade {name}`, then run `stado network up` on it",
    test: "tests/fleet/enroll_failures.rs::enroll_reports_a_machine_whose_stado_is_another_version",
    ..PLAIN
};
