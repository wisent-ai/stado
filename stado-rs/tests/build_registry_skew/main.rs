//! A host whose installed build refuses the registry the control plane
//! publishes.
//!
//! The last gap of the defect class `registry doctor` grew two other checks
//! for. On `lukasz-macbook` the disk janitor recorded `policy:ValueError`
//! 8,348 times across roughly 12,700 passes in two windows —
//! 2026-08-20T20:18:05Z to 2026-08-27T18:03:59Z, with zero successful policy
//! resolutions anywhere inside it, and 2026-08-31T06:30:49Z to
//! 2026-09-02T17:50:40Z. The registry was valid throughout; the running build
//! was too old to accept it. Both windows opened with no restart and no binary
//! replacement, and both closed on an unrelated restart onto a newer build.
//!
//! That is why `stale-unit-image` (#336) fires nothing here: the installed
//! file and the running image agreed, and the REGISTRY was what moved. The
//! janitor learned to journal the refusal as `policy:NotImplementedError`
//! (#341) and `resolver status` learned to publish it as a blocker for the
//! resolver's own process (#345), but the surface that carries every other
//! kind of drift said nothing.
//!
//! Measured on 2026-09-03 by asking each build on this machine to validate the
//! live registry: 0.7.14, 0.7.15, 0.7.16, 0.7.17 and 0.7.22 refuse it with
//! `registry.targets[0].disk_cleanup.cleaners: unknown cleaners
//! ['backup_twins', 'queue_workdirs']`, 0.13.24 refuses it with
//! `registry.targets[2].disk_cleanup: must contain exactly [...]`, and 0.13.46
//! onward accept it. Three build eras, three sentences, one class.
//!
//! What is defended here: the condition fires from inside the refusing process
//! — which is the whole case, and is only possible because `read_registry`
//! does not gate on `validate_registry`; the row names the host, the build and
//! the validator's own words rather than saying "incompatible"; it reuses the
//! `rejected-by-this-build` slug `resolver status` already publishes instead of
//! inventing a second vocabulary; a build that accepts is silent; every host
//! this process cannot ask gets exactly one row saying so, because unreadable
//! rendered as clean is the defect this whole line of work exists to remove;
//! and judging a document records and writes nothing, so a reporting surface
//! may call it.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde_json::Value;
use stado::targets::{
    build_refusal, builds_refusing_registry, last_good_refusal, running_build, BuildRegistrySkew,
    BuildVerdict, LastGoodRefusal, REGISTRY_LAST_GOOD_FILE, REGISTRY_LAST_GOOD_META_FILE,
};

/// The host this process is pretending to run on.
const LOCAL: &str = "macbook-fake";
/// A second declared machine, which no process here can ask.
const REMOTE: &str = "mini-fake";

const REFUSES: &str = "build-refuses-registry";
const UNREAD: &str = "unread-build-verdict";

/// Serializes `HOME`, which the no-write case has to own exclusively.
static HOME_LOCK: Mutex<()> = Mutex::new(());

/// A cache location this test owns, with `HOME` pointed at it for as long as
/// the guard lives. Same shape as `tests/registry_cache_refusal`, for the same
/// reason: `HOME` decides the cache location and is process-wide.
struct Home {
    _lock: MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}

impl Home {
    fn new() -> Self {
        let lock = HOME_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("temp HOME");
        let previous = std::env::var_os("HOME");
        std::env::set_var("HOME", dir.path());
        Self {
            _lock: lock,
            dir,
            previous,
        }
    }

    fn cache(&self) -> PathBuf {
        self.dir.path().join(".stado").join("cache")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(previous) => std::env::set_var("HOME", previous),
            None => std::env::remove_var("HOME"),
        }
    }
}

/// One local host, the full `disk_cleanup` field set, one cleaner this build
/// implements. `validate_registry` accepts it, which is what makes every
/// mutation below attributable to the mutation.
fn accepted() -> Value {
    serde_json::from_str(
        r#"{
        "schema_version": 2,
        "coordinators": [],
        "targets": [
            {
                "name": "macbook-fake",
                "kind": "local",
                "ssh": "u@10.0.0.1",
                "release_platform": "darwin-arm64",
                "hostnames": ["macbook-fake.local"],
                "disk_cleanup": {
                    "mode": "report",
                    "check_interval_seconds": 3600,
                    "low_free_gb": 100,
                    "target_free_gb": 200,
                    "max_bytes_per_pass": 68719476736,
                    "max_items_per_pass": 512,
                    "max_scan_items": 4096,
                    "cleaners": { "build_caches": { "min_age_seconds": 86400 } }
                }
            }
        ]
    }"#,
    )
    .expect("fixture parses")
}

/// The same document declaring one cleaner no build implements — the shape
/// 0.7.14 through 0.7.22 refuse today's live registry with, reproduced against
/// a name no build will ever implement so the case cannot decay into a pass
/// the day the cleaner lands.
fn declares_an_unimplemented_cleaner() -> Value {
    let mut document = accepted();
    document["targets"][0]["disk_cleanup"]["cleaners"]["queue_workdirs_fake"] =
        serde_json::json!({ "min_age_seconds": 86400 });
    document
}

/// The refusal, or a panic naming what came back instead.
fn refusal(skew: &BuildRegistrySkew) -> &LastGoodRefusal {
    match &skew.verdict {
        BuildVerdict::Refused(refusal) => refusal,
        BuildVerdict::Unmeasured { reason } => {
            panic!(
                "expected a refusal for {}, got unmeasured: {reason}",
                skew.host
            )
        }
    }
}

/// The whole case: the process holding the refusing build is the one that
/// reports it.
///
/// It is detectable there only because `read_registry` never gates on
/// `validate_registry` — `fetch_registry_remote_uncached` loads the published
/// document through `load_registry_from_str`, which skips what it cannot
/// model, and only the last-known-good cache is held to the contract. So the
/// refusing build reads the document, answers every other question in
/// `registry doctor`, and can be asked directly what it thinks of it.
#[test]
fn a_build_that_refuses_the_document_reports_it_about_itself() {
    let skews = builds_refusing_registry(LOCAL, &declares_an_unimplemented_cleaner(), Some(LOCAL));
    assert_eq!(skews.len(), 1, "one row for the one host that was asked");
    let skew = &skews[0];
    assert_eq!(skew.kind(), REFUSES);
    assert_eq!(skew.host, LOCAL);
    assert_eq!(
        skew.build.as_deref(),
        Some(running_build()),
        "the row names the build that answered, not an unread version"
    );
    assert!(matches!(
        refusal(skew),
        LastGoodRefusal::RejectedByThisBuild { .. }
    ));
}

/// A row saying "incompatible" is the same defect in a new place. The sentence
/// has to carry the host, the build and the validator's own words.
#[test]
fn the_row_names_the_host_the_build_and_the_rejection() {
    let document = declares_an_unimplemented_cleaner();
    let skews = builds_refusing_registry(LOCAL, &document, Some(LOCAL));
    let skew = &skews[0];
    let sentence = skew.sentence();
    assert!(sentence.contains(LOCAL), "names the host: {sentence}");
    assert!(
        sentence.contains(running_build()),
        "names the build version: {sentence}"
    );
    let produced = stado::targets::validate_registry(&document)
        .expect_err("the fixture is refused")
        .to_string();
    assert!(
        sentence.contains(&produced),
        "carries the validator's own sentence {produced:?}: {sentence}"
    );
    assert!(
        sentence.contains("queue_workdirs_fake"),
        "and therefore names what was refused: {sentence}"
    );
}

/// One fault, one word. `resolver status` publishes `rejected-by-this-build`
/// for the resolver's own process; this row must not invent a second name for
/// the same refusal.
#[test]
fn the_row_reuses_the_published_refusal_slug() {
    let skews = builds_refusing_registry(LOCAL, &declares_an_unimplemented_cleaner(), Some(LOCAL));
    let slug = refusal(&skews[0]).kind();
    assert_eq!(slug, "rejected-by-this-build");
    assert!(
        skews[0].sentence().contains(slug),
        "and prints it, the way the resolver's blocker does"
    );
}

/// A build that accepts the document has nothing to report. Without this the
/// check is a permanent row, which is noise and not a check.
#[test]
fn a_build_that_accepts_the_document_is_silent() {
    assert!(build_refusal(&accepted()).is_none());
    assert!(builds_refusing_registry(LOCAL, &accepted(), Some(LOCAL)).is_empty());
}

/// A document that is not a registry at all is still a refusal, not a pass:
/// the fallback path treats `targets` it cannot model as an empty fleet, and
/// this check must not inherit that tolerance.
#[test]
fn a_document_no_build_would_accept_is_refused_too() {
    let mut document = accepted();
    document["targets"] = Value::String("not-a-list".to_string());
    let skews = builds_refusing_registry(LOCAL, &document, Some(LOCAL));
    assert_eq!(skews.len(), 1);
    assert_eq!(skews[0].kind(), REFUSES);
}

/// Whether a remote host's build accepts the registry is not knowable from
/// here, and is reported as unmeasured rather than omitted — one row per host,
/// exactly as `unread-unit-image` does for a pid this kernel does not hold.
#[test]
fn a_host_this_process_cannot_ask_is_reported_unmeasured() {
    for document in [accepted(), declares_an_unimplemented_cleaner()] {
        let skews = builds_refusing_registry(REMOTE, &document, Some(LOCAL));
        assert_eq!(skews.len(), 1, "one row, always, for a host not asked");
        let skew = &skews[0];
        assert_eq!(skew.kind(), UNREAD);
        assert_eq!(skew.host, REMOTE);
        assert!(
            skew.build.is_none(),
            "the version there is not knowable from here either"
        );
        let sentence = skew.sentence();
        assert!(sentence.contains(REMOTE), "names the host: {sentence}");
        assert!(
            sentence.contains("NOT reported as acceptance"),
            "and refuses to be read as clean: {sentence}"
        );
        assert!(
            sentence.contains(running_build()) && sentence.contains(LOCAL),
            "and says which build asked from where: {sentence}"
        );
    }
}

/// The local verdict must not leak onto a remote host. A refusing local build
/// reporting every declared machine as refusing would be a worse lie than the
/// silence it replaces.
#[test]
fn a_local_refusal_is_not_attributed_to_another_host() {
    let document = declares_an_unimplemented_cleaner();
    let remote = builds_refusing_registry(REMOTE, &document, Some(LOCAL));
    assert_eq!(remote[0].kind(), UNREAD);
    assert!(matches!(remote[0].verdict, BuildVerdict::Unmeasured { .. }));
}

/// A machine no registry target names can ask no build for anybody: it gets
/// the unmeasured row with that phrase, the same way `observe_unit_images`
/// words it.
#[test]
fn a_host_with_no_local_target_at_all_is_unmeasured() {
    let skews = builds_refusing_registry(LOCAL, &declares_an_unimplemented_cleaner(), None);
    assert_eq!(skews.len(), 1);
    assert_eq!(skews[0].kind(), UNREAD);
    assert!(
        skews[0]
            .sentence()
            .contains("a host no registry target names"),
        "{}",
        skews[0].sentence()
    );
}

/// The two kinds are distinguishable, which is the point of having two: an
/// operator filtering for one must never catch the other.
#[test]
fn the_two_kinds_are_distinct() {
    assert_ne!(REFUSES, UNREAD);
    let refused =
        builds_refusing_registry(LOCAL, &declares_an_unimplemented_cleaner(), Some(LOCAL));
    let unmeasured = builds_refusing_registry(REMOTE, &accepted(), Some(LOCAL));
    assert_ne!(refused[0].kind(), unmeasured[0].kind());
}

/// Judging a document is not caching one. This runs on a reporting surface, so
/// it must leave no cache file behind and must not overwrite the recorded
/// refusal `resolver status` and the fallback notice read.
#[test]
fn judging_a_document_writes_nothing_and_records_nothing() {
    let home = Home::new();
    let before = last_good_refusal();
    assert!(build_refusal(&declares_an_unimplemented_cleaner()).is_some());
    assert!(build_refusal(&accepted()).is_none());
    assert_eq!(
        last_good_refusal(),
        before,
        "the process-local record belongs to store_last_good, not to a report"
    );
    for name in [REGISTRY_LAST_GOOD_FILE, REGISTRY_LAST_GOOD_META_FILE] {
        assert!(
            !home.cache().join(name).exists(),
            "{name} must not appear: nothing here writes a cache"
        );
    }
}

/// And the refusal is readable with no cache history at all, which is what
/// separates this check from reading `last_good_refusal()`. That record exists
/// only if this process happened to take the uncached authority path; a check
/// that fires only when the cache was refreshed cannot fire in the case it
/// exists for.
#[test]
fn the_refusal_does_not_depend_on_the_cache_having_been_refreshed() {
    let skews = builds_refusing_registry(LOCAL, &declares_an_unimplemented_cleaner(), Some(LOCAL));
    assert_eq!(skews[0].kind(), REFUSES);
    assert!(matches!(
        refusal(&skews[0]),
        LastGoodRefusal::RejectedByThisBuild { .. }
    ));
}
