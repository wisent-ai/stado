//! Refused consumer declarations never leave a partially changed registry.
use std::fs;

use super::{evidence, run, CLIENT};
use crate::fixture::Policy;
use crate::resolution::GENERATION;
use crate::{free_port, said, Host, SERVICE, TARGET};

#[test]
fn refused_binding_changes_leave_the_consumer_and_registry_untouched() {
    let policy = Policy::patient(GENERATION);
    let host = Host::new(&policy.document());
    let evidence = evidence();
    let before = fs::read(host.registry_path()).unwrap();
    let collision = format!("127.0.0.1:{}", policy.adapter);
    let answer = run(
        &host,
        &evidence,
        &[
            "service",
            "directory",
            "consumer-add",
            SERVICE,
            CLIENT,
            "--target",
            TARGET,
            "--bind",
            &collision,
            "--json",
        ],
    );
    assert!(!answer.status.success(), "a colliding binding was accepted");
    assert!(
        said(&answer).contains("duplicate resolver bind"),
        "{}",
        said(&answer)
    );
    assert_eq!(fs::read(host.registry_path()).unwrap(), before);

    let bind = format!("127.0.0.1:{}", free_port());
    let answer = run(
        &host,
        &evidence,
        &[
            "service",
            "directory",
            "consumer-add",
            SERVICE,
            CLIENT,
            "--target",
            "unknown-resolver",
            "--bind",
            &bind,
            "--json",
        ],
    );
    assert!(
        !answer.status.success(),
        "an unknown resolver host was accepted"
    );
    assert!(
        said(&answer).contains("resolver target \"unknown-resolver\" is not registered"),
        "{}",
        said(&answer)
    );
    assert_eq!(fs::read(host.registry_path()).unwrap(), before);

    let answer = run(
        &host,
        &evidence,
        &[
            "service",
            "directory",
            "consumer-add",
            SERVICE,
            CLIENT,
            "--target",
            TARGET,
            "--json",
        ],
    );
    assert_eq!(
        answer.status.code(),
        Some(2),
        "the bind/target pair was not required"
    );
    assert_eq!(fs::read(host.registry_path()).unwrap(), before);
}
