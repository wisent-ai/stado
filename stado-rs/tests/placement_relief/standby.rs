//! What `stado placement standby` refuses before it touches a host, read
//! through the real binary against the shared isolated fleet.

use crate::support::{fleet, stado, stderr, stdout, MINI, PROFILE, RTX};

const REASON: &str = "the mini is short of memory and the workstation has headroom";

#[test]
fn an_unknown_profile_is_refused_by_name() {
    let store = fleet(MINI);
    let out = stado(
        store.path(),
        &[
            "placement",
            "standby",
            "no-such-profile",
            "--host",
            RTX,
            "--reason",
            REASON,
        ],
    );
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("declares no placement profile named \"no-such-profile\""),
        "{}",
        stderr(&out)
    );
}

#[test]
fn the_placed_host_cannot_stand_by_for_its_own_profile() {
    let store = fleet(MINI);
    let out = stado(
        store.path(),
        &[
            "placement",
            "standby",
            PROFILE,
            "--host",
            MINI,
            "--reason",
            REASON,
        ],
    );
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("a placed host cannot stand by for its own profile"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn an_empty_reason_is_refused() {
    let store = fleet(MINI);
    let out = stado(
        store.path(),
        &[
            "placement",
            "standby",
            PROFILE,
            "--host",
            RTX,
            "--reason",
            " ",
        ],
    );
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("--reason must say why this host must stand by"),
        "{}",
        stderr(&out)
    );
}

/// The profile's first service runs a release-controlled tree; a registry
/// with no release control for that product cannot roll it out anywhere, and
/// the pass says so before delivering anything.
#[test]
fn a_tree_nothing_release_controls_is_refused_before_any_delivery() {
    let store = fleet(MINI);
    let out = stado(
        store.path(),
        &[
            "placement",
            "standby",
            PROFILE,
            "--host",
            RTX,
            "--reason",
            REASON,
        ],
    );
    assert!(!out.status.success(), "{}", stdout(&out));
    let stderr = stderr(&out);
    assert!(
        stderr.contains("registry.release_control declares no product named \"brama\""),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("nothing rolls it out to {RTX}")),
        "{stderr}"
    );
}
