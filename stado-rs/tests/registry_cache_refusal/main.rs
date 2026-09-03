//! What the last-known-good registry cache says when it refuses a document.
//!
//! `store_last_good` refused silently until 2026-09-03: each refusal printed
//! one `[registry-cache] not recording …` line to stderr and the function
//! returned `()`, so neither caller — `targets::fetch_registry_remote_uncached`
//! and `cli::resolver::ResolverState::refresh` — could know the host had
//! stopped accepting copies. A host can sit a registry generation behind
//! indefinitely that way, and the fallback that exists for the next outage
//! quietly answers from whatever generation it last took.
//!
//! What is defended here: the three refusals are distinguishable from each
//! other and from success, the refused document is still never recorded and
//! the older copy is left byte-identical, a success still writes BOTH the
//! document and its sidecar, and a host with no cache location at all says so
//! rather than looking like a rejection.
//!
//! `HOME` decides the cache location and is process-wide, so every test takes
//! `HOME_LOCK` for as long as it owns `HOME`.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use stado::targets::{
    last_good_refusal, store_last_good, LastGoodRefusal, REGISTRY_LAST_GOOD_FILE,
    REGISTRY_LAST_GOOD_META_FILE,
};

/// Serializes `HOME`, which every case has to own exclusively.
static HOME_LOCK: Mutex<()> = Mutex::new(());

/// A cache location this test owns, with `HOME` pointed at it for as long as
/// the guard lives.
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

    /// `HOME` with no cache location at all: the variable is gone, so
    /// `registry_last_good_path` cannot answer.
    fn unset() -> Self {
        let home = Self::new();
        std::env::remove_var("HOME");
        home
    }

    fn document(&self) -> PathBuf {
        self.dir
            .path()
            .join(".stado")
            .join("cache")
            .join(REGISTRY_LAST_GOOD_FILE)
    }

    fn sidecar(&self) -> PathBuf {
        self.dir
            .path()
            .join(".stado")
            .join("cache")
            .join(REGISTRY_LAST_GOOD_META_FILE)
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
fn accepted() -> String {
    r#"{
        "schema_version": 2,
        "coordinators": [],
        "targets": [
            {
                "name": "c1",
                "kind": "local",
                "ssh": "u@10.0.0.1",
                "release_platform": "darwin-arm64",
                "hostnames": ["c1.local"],
                "slots": 1,
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
    }"#
    .to_string()
}

/// The same document with one cleaner name this build has no implementation
/// for — the shape `validate_registry` rejects.
fn declares_an_unknown_cleaner() -> String {
    accepted().replace(
        r#""cleaners": { "build_caches": { "min_age_seconds": 86400 } }"#,
        r#""cleaners": { "build_caches": { "min_age_seconds": 86400 }, "chromium_profiles": { "min_age_seconds": 86400 } }"#,
    )
}

/// Schema-valid and names no hosts: what the authority served for nine
/// minutes on 2026-08-31.
fn names_no_hosts() -> String {
    r#"{"schema_version": 2, "coordinators": [], "targets": []}"#.to_string()
}

#[test]
fn a_success_writes_the_document_and_its_sidecar() {
    let home = Home::new();
    let text = accepted();

    store_last_good(&text, "gen-1").expect("an accepted document is recorded");

    assert_eq!(
        std::fs::read_to_string(home.document()).expect("document written"),
        text,
        "the copy is the authority's own bytes"
    );
    let sidecar =
        std::fs::read_to_string(home.sidecar()).expect("the sidecar dates the document written");
    assert!(
        sidecar.contains("\"generation\":\"gen-1\""),
        "the sidecar names the generation it recorded: {sidecar}"
    );
    assert!(
        sidecar.contains("\"read_at\""),
        "the sidecar dates the copy: {sidecar}"
    );
}

#[test]
fn a_document_that_does_not_parse_is_refused_as_unparseable() {
    let home = Home::new();
    store_last_good(&accepted(), "gen-1").expect("seed");
    let seeded = std::fs::read_to_string(home.document()).expect("seeded");

    let refusal = store_last_good("{\"schema_version\": 2, \"targets\": [", "gen-2")
        .expect_err("a truncated document is refused");

    assert!(matches!(refusal, LastGoodRefusal::Unparseable { .. }));
    assert_eq!(refusal.kind(), "unparseable-document");
    assert_eq!(
        std::fs::read_to_string(home.document()).expect("still there"),
        seeded,
        "the older copy is left exactly as it was"
    );
}

#[test]
fn a_document_this_build_does_not_accept_is_refused_as_such() {
    let home = Home::new();
    store_last_good(&accepted(), "gen-1").expect("seed");
    let seeded = std::fs::read_to_string(home.document()).expect("seeded");

    let refusal = store_last_good(&declares_an_unknown_cleaner(), "gen-2")
        .expect_err("an unknown cleaner is refused");

    assert!(matches!(
        refusal,
        LastGoodRefusal::RejectedByThisBuild { .. }
    ));
    assert_eq!(refusal.kind(), "rejected-by-this-build");
    assert!(
        refusal.detail().contains("unknown cleaners"),
        "the detail keeps the contract's own words: {}",
        refusal.detail()
    );
    assert_eq!(
        std::fs::read_to_string(home.document()).expect("still there"),
        seeded,
        "the older copy is left exactly as it was"
    );
}

#[test]
fn a_document_naming_no_hosts_is_refused_while_the_copy_names_some() {
    let home = Home::new();
    store_last_good(&accepted(), "gen-1").expect("seed");
    let seeded = std::fs::read_to_string(home.document()).expect("seeded");

    let refusal =
        store_last_good(&names_no_hosts(), "gen-2").expect_err("an empty fleet is refused");

    match &refusal {
        LastGoodRefusal::WouldLoseRecordedHosts { held, detail } => {
            assert_eq!(*held, 1, "the refusal counts the hosts the copy names");
            assert!(
                detail.contains("naming no hosts"),
                "the detail keeps the safeguard's own words: {detail}"
            );
        }
        other => panic!("expected a host-losing refusal, got {other:?}"),
    }
    assert_eq!(refusal.kind(), "would-lose-recorded-hosts");
    assert_eq!(
        std::fs::read_to_string(home.document()).expect("still there"),
        seeded,
        "the safeguard keeps the copy that names hosts"
    );
}

/// The same empty document IS recordable when nothing is held: the rule is
/// about losing hosts, not about being empty, and a fresh install legitimately
/// declares none. Proves the refusal above is the safeguard and not a blanket
/// refusal of empty documents.
#[test]
fn a_document_naming_no_hosts_is_recorded_when_nothing_is_held() {
    let home = Home::new();

    store_last_good(&names_no_hosts(), "gen-1").expect("a fresh install is cacheable");

    assert_eq!(
        std::fs::read_to_string(home.document()).expect("document written"),
        names_no_hosts()
    );
}

#[test]
fn a_host_with_no_cache_location_says_so() {
    let _home = Home::unset();

    let refusal = store_last_good(&accepted(), "gen-1").expect_err("there is nowhere to write");

    assert_eq!(refusal, LastGoodRefusal::NoCacheLocation);
    assert_eq!(refusal.kind(), "no-cache-location");
}

/// Four outcomes, four slugs. "Not recorded" sent an operator nowhere; these
/// send them to the document, to this binary's version, to what the authority
/// just published, and to `HOME`.
#[test]
fn every_refusal_is_distinguishable_from_the_others() {
    let home = Home::new();
    store_last_good(&accepted(), "gen-1").expect("seed");

    let unparseable = store_last_good("{", "gen-2").expect_err("unparseable");
    let rejected = store_last_good(&declares_an_unknown_cleaner(), "gen-2").expect_err("rejected");
    let empty = store_last_good(&names_no_hosts(), "gen-2").expect_err("empty");
    drop(home);
    let nowhere = {
        let _unset = Home::unset();
        store_last_good(&accepted(), "gen-2").expect_err("nowhere")
    };

    let kinds = [
        unparseable.kind(),
        rejected.kind(),
        empty.kind(),
        nowhere.kind(),
    ];
    let mut unique = kinds.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        kinds.len(),
        "four outcomes, four slugs: {kinds:?}"
    );
    for kind in kinds {
        assert!(
            !kind.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
            "a slug is safe to publish: {kind}"
        );
    }
}

/// The process-local record the degraded-read notice reads. Nothing writes it
/// on a success, so a host that is keeping its copy current never claims a
/// refusal.
#[test]
fn a_success_leaves_no_refusal_recorded() {
    let _home = Home::new();
    store_last_good(&accepted(), "gen-1").expect("recorded");

    assert_eq!(last_good_refusal(), None);
}
