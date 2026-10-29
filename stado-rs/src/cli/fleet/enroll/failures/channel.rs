//! The causes an enrollment meets on the way to the machine: installing the
//! fleet's key there (`--install-key`), reading the channel key, and the SSH
//! session that probes the machine's identity.

use wisent_errors::{Cause, Code};

use super::{Step, PLAIN};

pub(crate) static ADOPT_KEY_MISSING: Cause = Cause {
    id: "adopt-key-missing",
    code: Code::NotFound,
    failed: "installing the fleet's public key into ~/.ssh/authorized_keys on {ssh}",
    cause: "the credential store holds no public key `stado-ssh-{name}` and none could be minted",
    observed: "{answer}",
    remedy: "mint the pair with `stado fleet key generate {name}`, then run the same command again",
    test: "tests/fleet/enroll_failures.rs::enroll_install_key_refuses_without_a_fleet_key",
    ..PLAIN
};

pub(crate) static ADOPT_UNREACHABLE: Cause = Cause {
    id: "adopt-unreachable",
    code: Code::InfraDown,
    failed: "installing the fleet's public key into ~/.ssh/authorized_keys on {ssh}",
    cause: "the credential store or the machine did not answer while the key was being installed",
    observed: "{answer}",
    remedy: "bring back what the answer names (`stado doctor` for the store; the machine's SSH server for the machine), then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_install_key_refuses_an_unreachable_machine",
    ..PLAIN
};

pub(crate) static ADOPT_TIMEOUT: Cause = Cause {
    id: "adopt-timeout",
    code: Code::Timeout,
    failed: "installing the fleet's public key into ~/.ssh/authorized_keys on {ssh}",
    cause: "the session that installs the key did not finish within its deadline",
    observed: "{answer}",
    remedy: "check that {ssh} answers (`ssh {ssh} true`), then run the same command again",
    test: "tests/fleet/enroll_failures.rs::enroll_install_key_refuses_when_the_session_times_out",
    ..PLAIN
};

pub(crate) static ADOPT_REFUSED: Cause = Cause {
    id: "adopt-session-refused",
    code: Code::Auth,
    failed: "installing the fleet's public key into ~/.ssh/authorized_keys on {ssh}",
    cause: "a credential was refused: no loaded agent key, own key or password opened {ssh}, or the credential store refused Stado's grant to read the public key",
    observed: "{answer}",
    remedy: "open {ssh} by hand once (`ssh {ssh} true`) with a key or password that works, or grant Stado's reader the key item the answer names, then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_install_key_refuses_when_no_session_opens",
    ..PLAIN
};

pub(crate) static ADOPT_WRITE_REFUSED: Cause = Cause {
    id: "adopt-write-refused",
    code: Code::Refused,
    failed: "installing the fleet's public key into ~/.ssh/authorized_keys on {ssh}",
    cause: "the machine did not take the key: ~/.ssh or ~/.ssh/authorized_keys there cannot be written by that account, or the session did not answer as the install does",
    observed: "{answer}",
    remedy: "make ~/.ssh (mode 700) and ~/.ssh/authorized_keys (mode 600) belong to that account, then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_install_key_refuses_an_unwritable_authorized_keys",
    ..PLAIN
};

pub(crate) static ADOPT_SETTING_INVALID: Cause = Cause {
    id: "adopt-setting-invalid",
    code: Code::Config,
    failed: "installing the fleet's public key into ~/.ssh/authorized_keys on {ssh}",
    cause: "a setting the install reads is malformed: the owner key file, or the credential store's selector",
    observed: "{answer}",
    remedy: "fix the setting the answer names (`stado config show` prints the credential store selector), then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_install_key_refuses_a_malformed_key_setting",
    ..PLAIN
};

pub(crate) static ADOPT_FAILED: Cause = Cause {
    id: "adopt-failed",
    code: Code::InfraDown,
    failed: "installing the fleet's public key into ~/.ssh/authorized_keys on {ssh}",
    cause: "the install answered with an error Stado does not classify further: a file that could not be written on this machine, a full disk",
    observed: "{answer}",
    remedy: "fix what the answer names (free space, the temporary directory's permissions), then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_install_key_refuses_when_its_key_file_cannot_be_written",
    ..PLAIN
};

/// `--install-key`, by the class its answer stated.
pub(crate) static KEY_INSTALL: Step = Step {
    missing: &ADOPT_KEY_MISSING,
    unreachable: &ADOPT_UNREACHABLE,
    timeout: &ADOPT_TIMEOUT,
    auth: &ADOPT_REFUSED,
    refused: &ADOPT_WRITE_REFUSED,
    invalid: &ADOPT_SETTING_INVALID,
    failed: &ADOPT_FAILED,
};

pub(crate) static CHANNEL_KEY_MISSING: Cause = Cause {
    id: "channel-key-missing",
    code: Code::NotFound,
    failed: "reading the channel key `stado-ssh-{name}` from the credential store",
    cause: "the credential store holds no key item `stado-ssh-{name}`",
    observed: "{answer}",
    remedy: "mint one with `stado fleet key generate {name}` or move an existing key in with `stado fleet key add {name} --from FILE`, then run again with --install-key so the machine holds its public half",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_without_a_channel_key",
    ..PLAIN
};

pub(crate) static KEY_STORE_UNREACHABLE: Cause = Cause {
    id: "key-store-unreachable",
    code: Code::InfraDown,
    failed: "reading the channel key `stado-ssh-{name}` from the credential store",
    cause: "the credential store did not answer and this machine holds no copy of the key",
    observed: "{answer}",
    remedy:
        "bring the credential store back (`stado doctor` names it), then run the same command again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_while_the_credential_store_is_down",
    ..PLAIN
};

pub(crate) static KEY_STORE_TIMEOUT: Cause = Cause {
    id: "key-store-timeout",
    code: Code::Timeout,
    failed: "reading the channel key `stado-ssh-{name}` from the credential store",
    cause: "the credential store did not answer within the wait's deadline",
    observed: "{answer}",
    remedy: "run the same command again; when it keeps timing out, `stado doctor` shows which credential store check is slow",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_when_the_credential_store_times_out",
    ..PLAIN
};

pub(crate) static KEY_STORE_REFUSED: Cause = Cause {
    id: "key-store-refused",
    code: Code::Auth,
    failed: "reading the channel key `stado-ssh-{name}` from the credential store",
    cause: "the credential store refused Stado's grant to read the key",
    observed: "{answer}",
    remedy: "grant Stado's consumer read of `stado-ssh-{name}` in the store the answer names, then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_when_the_credential_store_refuses_the_grant",
    ..PLAIN
};

pub(crate) static KEY_FILE_REFUSED: Cause = Cause {
    id: "key-file-refused",
    code: Code::Refused,
    failed: "reading the channel key for '{name}'",
    cause: "the owner key file named by STADO_HOST_SSH_KEY_FILE grants group or other permissions, so it is not used",
    observed: "{answer}",
    remedy: "`chmod 600` the file, or unset STADO_HOST_SSH_KEY_FILE to read the key from the credential store",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_an_owner_key_file_others_can_read",
    ..PLAIN
};

pub(crate) static KEY_SETTING_INVALID: Cause = Cause {
    id: "key-setting-invalid",
    code: Code::Config,
    failed: "reading the channel key for '{name}'",
    cause: "the key setting is malformed: STADO_HOST_SSH_KEY_FILE is empty, or the credential store's selector cannot be read",
    observed: "{answer}",
    remedy: "fix the setting the answer names; `stado config show` prints the credential store selector",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_an_empty_owner_key_file",
    ..PLAIN
};

pub(crate) static KEY_FILE_UNWRITABLE: Cause = Cause {
    id: "key-file-unwritable",
    code: Code::InfraDown,
    failed: "putting the channel key for '{name}' into a private file on this machine",
    cause: "the file could not be written: a full disk, or a temporary directory this account cannot write",
    observed: "{answer}",
    remedy: "free space or fix the temporary directory's permissions, then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_when_the_key_file_cannot_be_written",
    ..PLAIN
};

/// Reading the channel key, by the class its answer stated.
pub(crate) static CHANNEL_KEY: Step = Step {
    missing: &CHANNEL_KEY_MISSING,
    unreachable: &KEY_STORE_UNREACHABLE,
    timeout: &KEY_STORE_TIMEOUT,
    auth: &KEY_STORE_REFUSED,
    refused: &KEY_FILE_REFUSED,
    invalid: &KEY_SETTING_INVALID,
    failed: &KEY_FILE_UNWRITABLE,
};

pub(crate) static SSH_UNSTARTABLE: Cause = Cause {
    id: "ssh-unstartable",
    code: Code::Config,
    failed: "starting ssh to verify {ssh}",
    cause: "the ssh program could not be started on this machine",
    observed: "{answer}",
    remedy: "install OpenSSH's client so `ssh` is on this account's PATH, then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_without_an_ssh_client",
    ..PLAIN
};

pub(crate) static DESTINATION_UNRESOLVABLE: Cause = Cause {
    id: "destination-unresolvable",
    code: Code::NotFound,
    failed: "verifying '{name}' over SSH at {ssh}",
    cause: "the host part of {ssh} does not resolve from this machine",
    observed: "ssh answered: {answer}",
    remedy: "correct --ssh, or make the name resolve here (a DNS record, an /etc/hosts line, the overlay network's name)",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_destination_that_does_not_resolve",
    ..PLAIN
};

pub(crate) static CONNECTION_REFUSED: Cause = Cause {
    id: "ssh-connection-refused",
    code: Code::InfraDown,
    failed: "verifying '{name}' over SSH at {ssh}",
    cause: "the machine answered and refused the SSH port: no SSH server is listening there",
    observed: "ssh answered: {answer}",
    remedy: "switch the SSH server on there (macOS: System Settings › General › Sharing › Remote Login; Linux: `sudo systemctl enable --now ssh`), then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_machine_without_an_ssh_server",
    ..PLAIN
};

pub(crate) static DESTINATION_TIMEOUT: Cause = Cause {
    id: "destination-timeout",
    code: Code::Timeout,
    failed: "verifying '{name}' over SSH at {ssh}",
    cause: "{ssh} did not answer within ssh's connection timeout: the machine is off or asleep, or a firewall drops the port",
    observed: "ssh answered: {answer}",
    remedy: "wake the machine or let port 22 through to it from this machine, then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_machine_that_does_not_answer",
    ..PLAIN
};

pub(crate) static NO_ROUTE: Cause = Cause {
    id: "no-route",
    code: Code::InfraDown,
    failed: "verifying '{name}' over SSH at {ssh}",
    cause: "this machine has no network route to {ssh}",
    observed: "ssh answered: {answer}",
    remedy: "connect this machine to a network that reaches the other one (the same LAN, or the overlay network), then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_destination_with_no_route",
    ..PLAIN
};

pub(crate) static SSH_AUTH_REFUSED: Cause = Cause {
    id: "ssh-auth-refused",
    code: Code::Auth,
    failed: "verifying '{name}' over SSH at {ssh}",
    cause: "{ssh} refused the fleet's key: it is not in that account's ~/.ssh/authorized_keys",
    observed: "ssh answered: {answer}",
    remedy: "run the same command again with --install-key over a session you can already open, or send the machine's owner `stado fleet invite create --expires 1h --offline`",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_machine_that_does_not_hold_the_fleet_key",
    ..PLAIN
};

pub(crate) static HOST_KEY_CHANGED: Cause = Cause {
    id: "host-key-changed",
    code: Code::Refused,
    failed: "verifying '{name}' over SSH at {ssh}",
    cause: "the host key {ssh} presented is not the one this machine recorded for it, so it may not be the machine it was",
    observed: "ssh answered: {answer}",
    remedy: "confirm the machine was reinstalled or replaced, remove the old key with `ssh-keygen -R HOST`, then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_changed_host_key",
    ..PLAIN
};

pub(crate) static PROBE_COMMAND_FAILED: Cause = Cause {
    id: "probe-command-failed",
    code: Code::Refused,
    failed: "reading `{command}` on {ssh}",
    cause: "the session opened, but `{command}` failed there",
    observed: "exit {status}: {answer}",
    remedy: "make `hostname` and `uname` runnable in that account's login shell, then run again",
    test:
        "tests/fleet/enroll_failures.rs::enroll_refuses_a_machine_whose_shell_cannot_run_the_probe",
    ..PLAIN
};

pub(crate) static CHANNEL_FAILED: Cause = Cause {
    id: "channel-failed",
    code: Code::InfraDown,
    failed: "verifying '{name}' over SSH at {ssh}",
    cause: "ssh ended the session with a failure it stated in words other than the causes above",
    observed: "ssh answered: {answer}",
    remedy: "`ssh -v {ssh} hostname` from this machine shows the step that fails; fix it there, then run again",
    test: "tests/fleet/enroll_failures.rs::enroll_refuses_a_session_ssh_ends_with_another_failure",
    ..PLAIN
};

/// The cause of a probe that did not succeed, read from what OpenSSH said.
///
/// OpenSSH reports a failure of its own with exit status 255 and one stable
/// diagnostic per cause; that diagnostic is the upstream's answer, read the
/// way an HTTP status is, not a Stado sentence searched for a class. Any other
/// status is the remote command's own.
pub(crate) fn probe_failure(status: i32, answer: &str) -> &'static Cause {
    const SSH_FAILED: i32 = 255;
    if status != SSH_FAILED {
        return &PROBE_COMMAND_FAILED;
    }
    let said = |words: &[&str]| words.iter().any(|word| answer.contains(word));
    if said(&[
        "Could not resolve hostname",
        "Name or service not known",
        "nodename nor servname",
    ]) {
        &DESTINATION_UNRESOLVABLE
    } else if said(&["Connection refused"]) {
        &CONNECTION_REFUSED
    } else if said(&[
        "Connection timed out",
        "Operation timed out",
        "timed out during banner exchange",
    ]) {
        &DESTINATION_TIMEOUT
    } else if said(&["No route to host", "Network is unreachable"]) {
        &NO_ROUTE
    } else if said(&[
        "REMOTE HOST IDENTIFICATION HAS CHANGED",
        "Host key verification failed",
    ]) {
        &HOST_KEY_CHANGED
    } else if said(&["Permission denied", "Too many authentication failures"]) {
        &SSH_AUTH_REFUSED
    } else {
        &CHANNEL_FAILED
    }
}
