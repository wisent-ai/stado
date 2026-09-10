//! `stado space` against the real disk of the machine running the test.
//!
//! The previous area for this capability drove the binary against a tempdir
//! and stopped there: it asserted that a declaration parses and that a refusal
//! sentence is exact, and called that evidence for reading and reclaiming
//! space, so it was deleted in ff4be506 along with six other areas that never
//! touched a host. This is the same capability with the host put back.
//!
//! Every case here declares an isolated local registry whose one target names
//! THIS machine, so `deploy::host_channel::target_is_this_host` is true and the
//! production code runs the operating system's own tools on the operating
//! system's own filesystem. The assertions then compare what the product
//! reported against what the machine itself answers — `df -Pk /`, `du -sk`,
//! `stat`'s allocated blocks, `tmutil listlocalsnapshots /` — and against
//! state left on disk: the removed tree, the janitor's state document, the
//! audit record, the exit code. A fabricated, zeroed or copied figure fails.
//!
//! Nothing outside the case's own tempdir is ever reclaimed. Each reclamation
//! selects one stage by name (`--stage`), those stages sweep only roots below
//! the fixture's `HOME`, and the applying cases first read the dry run's paths
//! and refuse to continue unless every one of them is inside that tempdir.

mod cleaners;
mod fixture;
mod leased;
mod mechanism;
mod reclamation;
mod refusals;
mod system;
mod watermarks;

use std::collections::BTreeSet;
use std::fs;

use fixture::{Host, TARGET, UNDECLARED_TARGET};
use system::{df_root, du_bytes, local_snapshots};

/// `df`'s available figure moves while the test runs — this machine builds
/// Rust — so the two reads are compared inside a band rather than pinned. The
/// band is far tighter than the difference between a real answer and an
/// invented one: 4 GiB against a volume that reports 1.8 TiB.
const DRIFT_KB: i64 = 4 * 1024 * 1024;
/// `df -Pk` capacity is a whole percentage, so the two reads may differ by one
/// point around a rounding boundary.
const CAPACITY_DRIFT: i64 = 1;
const KIB: i64 = 1024;

/// The reported disk figures are this filesystem's own, read through the same
/// tool the product uses, so a zeroed or invented number cannot pass.
#[test]
fn reported_space_is_this_filesystems_own_answer() {
    let host = Host::new();
    let report = host.json(&["space", "report", TARGET, "--json"]);
    let df = df_root();
    let usage = &report["usage"];

    assert_eq!(usage["filesystem"], df.filesystem);
    assert_eq!(usage["mounted_on"], df.mounted_on);
    // Total size does not move while the test runs, so this is an equality.
    assert_eq!(usage["blocks_kb"], df.blocks_kb.to_string());

    let reported = |field: &str| -> i64 {
        usage[field]
            .as_str()
            .unwrap_or_else(|| panic!("usage.{field} is a string of blocks: {usage}"))
            .parse()
            .unwrap_or_else(|error| panic!("usage.{field} is not a block count: {error}"))
    };
    let available = reported("available_kb");
    let used = reported("used_kb");
    assert!(
        (available - df.available_kb).abs() < DRIFT_KB,
        "reported {available} KiB available; df says {} KiB",
        df.available_kb
    );
    assert!(
        (used - df.used_kb).abs() < DRIFT_KB,
        "reported {used} KiB used; df says {} KiB",
        df.used_kb
    );
    assert!(
        used > 0 && available > 0 && used + available <= df.blocks_kb,
        "reported figures are not a real filesystem: used {used}, available {available}"
    );

    let percent = |value: &str| -> i64 {
        value
            .trim_end_matches('%')
            .parse()
            .unwrap_or_else(|error| panic!("{value:?} is not a df capacity: {error}"))
    };
    let reported_capacity = percent(usage["capacity"].as_str().expect("capacity is a string"));
    assert!(
        (reported_capacity - percent(&df.capacity)).abs() <= CAPACITY_DRIFT,
        "reported capacity {reported_capacity}%; df says {}",
        df.capacity
    );

    // The watermark section an operator reads the pressure verdict from must
    // carry the same free space, in bytes, as the row above it.
    assert_eq!(
        report["free_space"]["available_bytes"].as_i64(),
        Some(available * KIB)
    );
}

/// What occupies this host is a real inventory: paths that exist, sizes that
/// are not all zero, and the local snapshots the machine is actually holding.
#[test]
fn the_inventory_names_real_occupants_of_this_host() {
    let host = Host::new();
    let report = host.json(&["space", "report", TARGET, "--json"]);

    let inventory = report["inventory"]
        .as_array()
        .expect("the report carries an inventory");
    assert!(!inventory.is_empty(), "the inventory is empty: {report}");
    let mut occupied = false;
    let mut outside_the_fixture = false;
    for item in inventory {
        let path = item["path"]
            .as_str()
            .expect("an inventory path is a string");
        assert!(path.starts_with('/'), "{path} is not an absolute path");
        let found = fs::symlink_metadata(path).unwrap_or_else(|error| {
            panic!("the inventory names {path}, which cannot be read: {error}")
        });
        assert!(
            found.is_dir(),
            "the inventory names {path}, which is not a directory"
        );
        let size = item["size_gb"]
            .as_f64()
            .expect("an inventory size is a number");
        assert!(size >= 0.0, "{path} is reported as {size} GiB");
        occupied |= size > 0.0;
        outside_the_fixture |= !path.starts_with(&*host.root.to_string_lossy());
    }
    assert!(
        occupied,
        "every measured directory reported zero, which no real host does: {report}"
    );
    assert!(
        outside_the_fixture,
        "the inventory named nothing but the fixture's own tempdir: {report}"
    );

    // Local APFS snapshots hold blocks inside `used` that nothing in this
    // product reclaims, so the report has to name the ones the host holds.
    let snapshots = &report["local_snapshots"];
    let held = local_snapshots();
    let named: BTreeSet<String> = snapshots["names"]
        .as_array()
        .expect("the report carries snapshot names")
        .iter()
        .map(|name| {
            name.as_str()
                .expect("a snapshot name is a string")
                .to_string()
        })
        .collect();
    assert_eq!(snapshots["count"].as_u64(), Some(named.len() as u64));
    for name in &named {
        assert!(
            held.contains(name),
            "the report names {name}, which tmutil does not: {held:?}"
        );
    }
    assert_eq!(
        named.is_empty(),
        held.is_empty(),
        "tmutil holds {held:?} while the report named {named:?}"
    );
}

/// The declared build-cache scope is measured with the host's own `du`, and
/// only a directory its build tool tagged is a candidate for removal.
#[test]
fn the_declared_cache_scope_is_measured_with_the_hosts_own_du() {
    let host = Host::new();
    let cache_root = host.cache_root.clone();
    let tagged = host.seed_tree(&cache_root, "target-tree", 8, true);
    let untagged = host.seed_tree(&cache_root, "someones-work", 4, false);
    let measured = du_bytes(&tagged);

    let report = host.json(&["space", "report", TARGET, "--json"]);
    let caches = &report["build_caches"];
    assert_eq!(
        caches["declaration"]["root"].as_str(),
        Some(&*cache_root.to_string_lossy())
    );

    let entries = caches["entries"]
        .as_array()
        .expect("the report carries entries");
    let entry = entries
        .iter()
        .find(|entry| entry["path"].as_str() == Some(&*tagged.to_string_lossy()))
        .unwrap_or_else(|| panic!("the tagged cache is not in the report: {entries:?}"));
    assert_eq!(entry["verdict"], "candidate");
    let reported: i64 = entry["kib"]
        .as_str()
        .expect("a cache size is a string of KiB")
        .parse()
        .expect("a cache size is a KiB count");
    assert_eq!(
        reported * KIB,
        measured,
        "the product reported {reported} KiB for {}; du says {} bytes",
        tagged.display(),
        measured
    );

    // Only what a build tool tagged as regenerable is a candidate. The
    // neighbour holds the same kind of payload under a plausible name and
    // must not appear in the report at all, because a name is not permission.
    assert!(
        !entries
            .iter()
            .any(|entry| entry["path"].as_str() == Some(&*untagged.to_string_lossy())),
        "an untagged directory was reported as a cache: {entries:?}"
    );
    assert!(
        untagged.join("payload.bin").is_file(),
        "reading the scope removed an untagged directory"
    );
}

/// A host that declares no cleanup policy is measured against the reporting
/// default, not refused.
///
/// One declaration cannot have two answers. The janitor has resolved an
/// undeclared host against `DiskCleanupPolicy::reporting_default` since the
/// `lukasz-macbook` space incident — silence in the registry means "nobody has
/// said", not "do not look" — while this reader refused with `declares no disk
/// cleanup policy`, so the same host was both reported on and unreadable
/// depending on which command asked. Nothing here arms a cleaner: the default
/// is `mode: report`, and the case proves the read happened by finding this
/// machine's own filesystem in the answer.
#[test]
fn a_host_that_declares_no_policy_is_read_against_the_reporting_default() {
    let host = Host::new();
    host.declare_no_scope();
    let tagged = host.seed_tree(&host.home.join("target"), "target-tree", 4, true);

    let report = host.json(&["space", "report", UNDECLARED_TARGET, "--json"]);
    assert_eq!(
        report["build_caches"]["declaration"]["root"].as_str(),
        Some(&*host.home.to_string_lossy()),
        "an undeclared host's cache scope is its home: {report}"
    );
    assert_eq!(
        report["usage"]["filesystem"].as_str(),
        Some(&*df_root().filesystem),
        "the read did not reach this machine's filesystem: {report}"
    );
    let entries = report["build_caches"]["entries"]
        .as_array()
        .expect("the report carries entries");
    assert!(
        entries
            .iter()
            .any(|entry| entry["path"].as_str() == Some(&*tagged.to_string_lossy())),
        "the default scope did not reach the tagged cache under this home: {entries:?}"
    );
    // Reporting, never deleting: the default the fleet resolves an undeclared
    // host against says `report`, and an operator who declared nothing has not
    // asked for a janitor.
    assert!(
        tagged.join("payload.bin").is_file(),
        "reading an undeclared host's caches removed one"
    );
}
