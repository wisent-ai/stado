//! `stado space cleaners` against a real registry document.
//!
//! Each case drives the built binary with the area's isolated fixture, then
//! reads the state the command was supposed to change: the canonical registry
//! document on disk. The refusal sentences are part of the contract, so they
//! are asserted verbatim.
//!
//! The story these defend is one incident. `charless-mac-mini` sat below its
//! declared target with 52.5 GiB under `~/.stado/local-storage` and 10.5 GiB
//! under `~/.stado/local-backup`, and `stado space report` said no declared
//! stage looked at either — while `release_store` and `backup_twins`, which
//! sweep exactly those roots, were declared on that host. Arming a cleaner had
//! to become a typed write rather than a hand edit of the registry, and the
//! write's two refusals are the whole of its safety.

use std::fs;

use serde_json::Value;

use crate::fixture::{Host, TARGET};
use crate::system::said;

/// A `stado` recent enough for every cleaner in the catalogue.
const CURRENT: &str = "0.16.38";
/// A `stado` that predates `release_store` and knows every other cleaner.
const BEFORE_RELEASE_STORE: &str = "0.15.0";

/// The registry document the fixture's storage holds right now.
fn registry(host: &Host) -> Value {
    let raw =
        fs::read_to_string(host.storage.join("registry.json")).expect("read fixture registry");
    serde_json::from_str(&raw).expect("fixture registry is JSON")
}

/// The cleaners the one target declares, as the document on disk has them.
fn declared(host: &Host) -> Value {
    registry(host)["targets"][0]["disk_cleanup"]["cleaners"].clone()
}

/// One row of `space cleaners list --json`.
fn row(listing: &Value, cleaner: &str) -> Value {
    listing["cleaners"]
        .as_array()
        .expect("the listing carries rows")
        .iter()
        .find(|row| row["cleaner"] == cleaner)
        .unwrap_or_else(|| panic!("{cleaner} is not in the listing"))
        .clone()
}

#[test]
fn every_implemented_cleaner_is_listed_against_what_the_host_declares() {
    let host = Host::new();
    host.declare_running(&host.policy(), CURRENT);

    let listing = host.json(&["space", "cleaners", "list", TARGET, "--json"]);
    assert_eq!(listing["installed_stado"], CURRENT);
    assert_eq!(
        listing["cleaners"].as_array().map(Vec::len),
        Some(7),
        "the listing must name every cleaner this product implements: {listing}"
    );

    let armed = row(&listing, "build_caches");
    assert_eq!(armed["declared"], Value::Bool(true));
    assert_eq!(
        armed["declaration"]["root"],
        Value::from(host.cache_root.to_string_lossy().to_string()),
        "a declared row must carry the declaration the host actually holds"
    );

    // The one the host does not declare says so, and says what to run.
    let idle = row(&listing, "release_store");
    assert_eq!(idle["declared"], Value::Bool(false));
    assert_eq!(idle["supported_by_installed_binary"], Value::Bool(true));
    assert_eq!(
        idle["detail"].as_str().unwrap_or_default(),
        "this product implements it and this host does not declare it; arm it with `stado space cleaners declare <target> --cleaner release_store`"
    );
    assert_eq!(
        idle["default_root"], ".stado/local-storage/ecosystem/releases",
        "the row must name where the cleaner would sweep"
    );
}

#[test]
fn declaring_a_cleaner_writes_it_and_withdrawing_it_takes_it_back() {
    let host = Host::new();
    host.declare_running("null", CURRENT);
    assert!(
        registry(&host)["targets"][0]["disk_cleanup"].is_null(),
        "this case starts from a host that declares no policy at all"
    );

    let written = host.json(&[
        "space",
        "cleaners",
        "declare",
        TARGET,
        "--cleaner",
        "release_store",
        "--keep-newest",
        "2",
        "--json",
    ]);
    assert_eq!(written["cleaner"], "release_store");

    // The document on disk is the assertion, not the command's own output.
    assert_eq!(declared(&host)["release_store"]["keep_newest"], 2);
    let policy = registry(&host)["targets"][0]["disk_cleanup"].clone();
    assert!(
        policy["low_free_gb"].as_i64().is_some_and(|value| value > 0),
        "a host that declared nothing must be seeded with the default it was already measured against, not with a bare cleaner: {policy}"
    );

    let listing = host.json(&["space", "cleaners", "list", TARGET, "--json"]);
    assert_eq!(
        row(&listing, "release_store")["declared"],
        Value::Bool(true)
    );

    host.json(&[
        "space",
        "cleaners",
        "remove",
        TARGET,
        "--cleaner",
        "release_store",
        "--json",
    ]);
    assert!(
        declared(&host)["release_store"].is_null(),
        "withdrawing must remove the key: {}",
        declared(&host)
    );

    let refused = host.run(&[
        "space",
        "cleaners",
        "remove",
        TARGET,
        "--cleaner",
        "release_store",
    ]);
    assert!(!refused.status.success(), "withdrawing twice must refuse");
    assert!(
        said(&refused.stderr).contains(&format!("{TARGET} declares no cleaner release_store")),
        "{}",
        said(&refused.stderr)
    );
}

#[test]
fn a_cleaner_this_product_does_not_implement_is_refused_with_the_list() {
    let host = Host::new();
    host.declare_running(&host.policy(), CURRENT);

    let refused = host.run(&[
        "space",
        "cleaners",
        "declare",
        TARGET,
        "--cleaner",
        "rm_rf_home",
    ]);
    assert!(!refused.status.success());
    let sentence = said(&refused.stderr);
    assert!(
        sentence.contains(
            "rm_rf_home is not a cleaner this product implements; declare one of: \
             backup_twins, build_caches, chromium_clones, huggingface_cache, \
             queue_workdirs, release_store, weles_recordings"
        ),
        "{sentence}"
    );
    assert!(
        declared(&host)["rm_rf_home"].is_null(),
        "a refused name must never reach the document"
    );
}

#[test]
fn a_cleaner_the_installed_binary_predates_is_refused_before_the_write() {
    let host = Host::new();
    host.declare_running(&host.policy(), BEFORE_RELEASE_STORE);

    let refused = host.run(&[
        "space",
        "cleaners",
        "declare",
        TARGET,
        "--cleaner",
        "release_store",
    ]);
    assert!(
        !refused.status.success(),
        "declaring a cleaner the host cannot parse must refuse: that write switched off every cleaner on a real host"
    );
    let sentence = said(&refused.stderr);
    assert!(
        sentence.contains(&format!(
            "{TARGET} runs stado {BEFORE_RELEASE_STORE} and release_store first ships in 0.15.26"
        )),
        "{sentence}"
    );
    assert!(
        declared(&host)["release_store"].is_null(),
        "the refusal must leave the document untouched: {}",
        declared(&host)
    );
    assert_eq!(
        row(
            &host.json(&["space", "cleaners", "list", TARGET, "--json"]),
            "release_store"
        )["supported_by_installed_binary"],
        Value::Bool(false),
        "and the listing must say why"
    );
}
