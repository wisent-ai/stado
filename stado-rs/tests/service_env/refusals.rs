//! What `service env-show` and `env-set` refuse, in the words a live run of
//! the built binary printed on this machine.
//!
//! Every sentence below was copied from `probe.sh`'s output against this same
//! registry shape, and every refusal is checked against the file on disk
//! afterwards: a refused read shows nothing it was not allowed to read, and a
//! refused write leaves the env file exactly as the test wrote it.

use crate::{on_disk, stderr, stdout, Fleet, OWNER_ONLY};

#[test]
fn env_show_refuses_a_key_that_is_not_an_environment_variable_name() {
    let fleet = Fleet::new();
    let path = fleet.env_file("WELES_QUEUE=default\n");

    let out = fleet.env_show(path.to_str().unwrap(), &["--reveal", "weles_queue"]);
    assert!(!out.status.success(), "env-show accepted a lowercase key");
    assert!(
        stderr(&out).contains("--key must be an uppercase environment variable name"),
        "got: {}",
        stderr(&out)
    );
    assert!(
        !stdout(&out).contains("default"),
        "the file was read anyway:\n{}",
        stdout(&out)
    );
}

#[test]
fn env_set_refuses_a_dotted_key_before_the_file_is_touched() {
    let fleet = Fleet::new();
    // A dotted key is how a plist or a systemd drop-in spells a setting, and
    // it is not an environment variable name. Accepting it would append a
    // line no shell that sources this file could ever assign.
    let path = fleet.env_file("WELES_QUEUE=default\n");
    let before = on_disk(&path);

    let out = fleet.env_set("WELES.QUEUE", path.to_str().unwrap(), "batch");
    assert!(!out.status.success(), "env-set accepted a dotted key");
    assert!(
        stderr(&out).contains("--key must be an uppercase environment variable name"),
        "got: {}",
        stderr(&out)
    );
    assert_eq!(
        on_disk(&path),
        before,
        "a refused key still changed the file"
    );
}

#[test]
fn env_set_refuses_a_value_file_holding_more_than_one_value() {
    let fleet = Fleet::new();
    let path = fleet.env_file("WELES_QUEUE=default\n");
    let before = on_disk(&path);
    // Two lines: an env file assigns one value per line, so writing this
    // would turn the second line into an assignment nobody declared.
    let value = fleet.value_file("two-line-value", "first\nsecond\n", OWNER_ONLY);

    let out = fleet.env_set_from(
        "WELES_QUEUE",
        path.to_str().unwrap(),
        value.to_str().unwrap(),
    );
    assert!(!out.status.success(), "env-set accepted a two-line value");
    assert!(
        stderr(&out).contains(&format!(
            "{} must contain one non-empty value",
            value.display()
        )),
        "got: {}",
        stderr(&out)
    );
    assert_eq!(
        on_disk(&path),
        before,
        "a refused value still changed the file"
    );
}

#[test]
fn env_set_refuses_a_value_file_others_can_read() {
    let fleet = Fleet::new();
    let path = fleet.env_file("WELES_QUEUE=default\n");
    let before = on_disk(&path);
    // The value is about to become a secret in a managed file; a world- or
    // group-readable staging file has already leaked it.
    let value = fleet.value_file("group-readable-value", "batch\n", 0o644);

    let out = fleet.env_set_from(
        "WELES_QUEUE",
        path.to_str().unwrap(),
        value.to_str().unwrap(),
    );
    assert!(
        !out.status.success(),
        "env-set read a value out of a file others can read"
    );
    assert!(
        stderr(&out).contains(&format!("{} must be owner-only", value.display())),
        "got: {}",
        stderr(&out)
    );
    assert_eq!(
        on_disk(&path),
        before,
        "a refused value still changed the file"
    );
}

#[test]
fn env_show_refuses_a_host_the_registry_does_not_hold() {
    let fleet = Fleet::new();
    let path = fleet.env_file("WELES_QUEUE=default\n");

    // `elsewhere` is a name this registry never declared. A reader that fell
    // back to the machine it is standing on would read this file and report
    // it as another host's.
    let out = fleet.service("env-show", "elsewhere", path.to_str().unwrap(), &[]);
    assert!(
        !out.status.success(),
        "env-show answered for a host outside the registry:\n{}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("target 'elsewhere' is not in the canonical registry"),
        "got: {}",
        stderr(&out)
    );
    assert!(
        !stdout(&out).contains("WELES_QUEUE"),
        "the file was read for an unknown host:\n{}",
        stdout(&out)
    );
}

#[test]
fn env_show_refuses_a_symlink_without_following_it() {
    let fleet = Fleet::new();
    let real = fleet.env_file("WELES_QUEUE=default\n");
    let link = fleet.home.path().join(".config/weles/worker.env.link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let out = fleet.env_show(link.to_str().unwrap(), &[]);
    assert!(!out.status.success(), "env-show followed a symlink");
    assert!(
        stderr(&out).contains("refused_symlink — the target is a symlink and was not followed"),
        "got: {}",
        stderr(&out)
    );
    assert!(
        !stdout(&out).contains("WELES_QUEUE"),
        "the symlink's target was read anyway:\n{}",
        stdout(&out)
    );
}

#[test]
fn env_show_refuses_a_path_outside_the_target_home() {
    let fleet = Fleet::new();
    fleet.env_file("WELES_QUEUE=default\n");

    let out = fleet.env_show("/etc/hosts", &[]);
    assert!(!out.status.success(), "env-show read outside the home");
    assert!(
        stderr(&out).contains("refused_outside_home — the target must be inside the target home"),
        "got: {}",
        stderr(&out)
    );
    // /etc/hosts on this machine names localhost; none of it came back.
    assert!(
        !stdout(&out).contains("localhost"),
        "a file outside the home was read anyway:\n{}",
        stdout(&out)
    );
}

#[test]
fn env_show_reports_a_file_that_is_not_there_rather_than_an_empty_one() {
    let fleet = Fleet::new();
    let missing = fleet.home.path().join(".config/weles/absent.env");

    let out = fleet.env_show(missing.to_str().unwrap(), &[]);
    assert!(!out.status.success(), "env-show invented a file");
    assert!(
        stderr(&out).contains("missing — no regular file at the target"),
        "got: {}",
        stderr(&out)
    );
    // Reporting a file as missing must not create it.
    assert!(!missing.exists(), "the read created {}", missing.display());
}
