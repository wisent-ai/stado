//! The other paths of the same commands: what a lease refuses, and in which
//! words. Each sentence is part of the contract, so each is asserted whole.

use super::fleet::{document, leasable_host, profiles, run, stderr};

/// A profile the declaration does not carry is named as such, with the ones it
/// does carry, so nobody has to open the document to find out.
#[test]
fn an_undeclared_profile_is_refused_with_the_declared_ones() {
    let host = leasable_host();
    let output = run(&[
        "scratch",
        "create",
        "--host",
        &host.target,
        "--profile",
        "macos-vm",
    ]);
    assert!(!output.status.success(), "an undeclared profile is refused");
    let declared = profiles()
        .iter()
        .filter_map(|profile| profile["name"].as_str().map(str::to_string))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(
        stderr(&output).contains(&format!(
            "scratch profile 'macos-vm' is not declared in stado-rs/data/scratch-profiles.json; \
             declared profiles: {declared}"
        )),
        "the refusal names the declaration and the declared profiles: {}",
        stderr(&output)
    );
}

/// A profile that covers other platforms is refused against this host, naming
/// both sides — the profile's platforms and what the target says it runs.
#[test]
fn a_profile_that_does_not_cover_the_platform_is_refused() {
    let host = leasable_host();
    let Some(other) = profiles().into_iter().find(|profile| {
        profile["name"].as_str() != Some(host.profile.as_str())
            && !profile["platforms"].as_array().is_some_and(|platforms| {
                platforms
                    .iter()
                    .any(|platform| platform.as_str() == Some(host.release_platform.as_str()))
            })
    }) else {
        panic!(
            "the declaration carries no profile that excludes {}",
            host.release_platform
        );
    };
    let name = other["name"].as_str().expect("a profile name");
    let platforms = other["platforms"]
        .as_array()
        .expect("declared platforms")
        .iter()
        .filter_map(|platform| platform.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let output = run(&[
        "scratch",
        "create",
        "--host",
        &host.target,
        "--profile",
        name,
    ]);
    assert!(!output.status.success(), "a platform mismatch is refused");
    assert!(
        stderr(&output).contains(&format!(
            "profile '{name}' is declared for platforms {platforms}; target '{}' declares release_platform '{}'",
            host.target, host.release_platform
        )),
        "the refusal names the profile's platforms and the target's: {}",
        stderr(&output)
    );
}

/// A lifetime above the profile's ceiling is refused in the profile's own
/// vocabulary, not in minutes.
#[test]
fn a_lifetime_above_the_ceiling_is_refused_in_the_declared_words() {
    let host = leasable_host();
    let profile = profiles()
        .into_iter()
        .find(|profile| profile["name"].as_str() == Some(host.profile.as_str()))
        .expect("the host's profile is declared");
    let ceiling = profile["max_ttl"].as_str().expect("a declared maximum");
    let output = run(&[
        "scratch",
        "create",
        "--host",
        &host.target,
        "--profile",
        &host.profile,
        "--ttl",
        "12h",
    ]);
    assert!(
        !output.status.success(),
        "a lifetime over the ceiling is refused"
    );
    assert!(
        stderr(&output).contains(&format!(
            "profile '{}' allows at most {ceiling}; 12h was requested",
            host.profile
        )),
        "the refusal quotes the declared ceiling and the request: {}",
        stderr(&output)
    );
}

/// The lease name is also the account name, so the name rule is the account
/// rule, and a name outside it is refused before anything reaches the host.
#[test]
fn a_name_outside_the_rule_is_refused_before_the_host_is_touched() {
    let host = leasable_host();
    let output = run(&[
        "scratch",
        "create",
        "--host",
        &host.target,
        "--profile",
        &host.profile,
        "--name",
        "Scratch_One",
    ]);
    assert!(
        !output.status.success(),
        "a name outside the rule is refused"
    );
    assert!(
        stderr(&output).contains(
            "scratch names are lowercase [a-z0-9-] beginning with a letter; 'Scratch_One' is not"
        ),
        "the refusal states the rule and the name: {}",
        stderr(&output)
    );
}

/// Destroying something the host does not hold says so, and says on which host.
#[test]
fn a_lease_nobody_holds_cannot_be_destroyed() {
    let host = leasable_host();
    let output = run(&[
        "scratch",
        "destroy",
        "scratch-nobodyholds",
        "--host",
        &host.target,
    ]);
    assert!(!output.status.success(), "an unknown lease is refused");
    assert!(
        stderr(&output).contains(&format!(
            "no scratch lease named 'scratch-nobodyholds' on '{}'",
            host.target
        )),
        "the refusal names the lease and the host: {}",
        stderr(&output)
    );
}

/// The reader that decides where a lease may be taken: every leasable row
/// carries the profile and route that make it leasable, and every refused row
/// carries the reason instead. A row with neither would send a caller to a host
/// that cannot answer.
#[test]
fn every_host_row_carries_either_a_profile_or_a_refusal() {
    let arguments = ["scratch", "hosts", "--json"];
    let output = run(&arguments);
    let report = document(&output, &arguments);
    let hosts = report["hosts"].as_array().expect("a hosts array");
    assert!(!hosts.is_empty(), "the registry declares targets: {report}");
    for row in hosts {
        let target = row["target"].as_str().unwrap_or_default();
        if row["eligible"].as_bool().unwrap_or_default() {
            assert!(
                row["profile"].as_str().is_some_and(|name| !name.is_empty()),
                "{target} is leasable, so a profile covers it: {row}"
            );
            assert!(
                row["ssh"].as_str().is_some_and(|route| !route.is_empty()),
                "{target} is leasable, so the registry gives it a route: {row}"
            );
            assert_eq!(row["refusal"], serde_json::Value::Null);
        } else {
            assert!(
                row["refusal"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty()),
                "{target} is not leasable, so the report says why: {row}"
            );
        }
    }
    assert_eq!(
        report["eligible"],
        serde_json::Value::from(
            hosts
                .iter()
                .filter(|row| row["eligible"].as_bool().unwrap_or_default())
                .count()
        ),
        "the count matches the rows: {report}"
    );
}
