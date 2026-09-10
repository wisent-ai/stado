//! A real disk-cleanup pass must never delay a capacity publication.
//!
//! `CAPACITY_STALE_SECONDS` is three times `CAPACITY_HEARTBEAT_INTERVAL_S`, and
//! `release_submit::builder` refuses outright when no fresh publication names
//! the platform, so a host that stops publishing stops being a release builder
//! fleet-wide while staying healthy. The tick used to `await run_cleanup_once`
//! before publishing, on the same task: charless-mac-mini, 2026-09-03, a
//! `healthy_noop` pass costing 818021 ms against a 300-second interval, so two
//! weles-worker releases were refused against a builder that was up.
//!
//! The pass below is the product's own engine, `run_cleanup_once` under the
//! `AgentTick` identity — the call `providers::local::agent` makes. It resolves
//! a declared policy from a real registry document, takes its own cross-process
//! lock, walks a real tree under its real `CACHEDIR.TAG` rules, deletes inside
//! its declared budget and persists its own state file; every duration below
//! was spent by it and read from its report. It replaced a pass that only
//! slept, which cannot be slow the way a walk and a store read are. Isolation
//! is of DATA: a scratch `HOME`, store and cleaner root; heartbeat is scaled to
//! milliseconds so the constants' ratio needs no 180-second test.

use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use stado::constants::{CAPACITY_HEARTBEAT_INTERVAL_S, CAPACITY_STALE_SECONDS};
use stado::providers::local::agent::janitor::{JanitorReports, JanitorTask};
use stado::providers::local::disk_cleanup::{run_cleanup_once, CleanupWriter};

/// The test's scaled heartbeat; every other duration is a multiple of it.
const HEARTBEAT: Duration = Duration::from_millis(20);

const PASS_HEARTBEATS: u32 = 10;
const PASS_DEADLINE: Duration = Duration::from_secs(60);

/// The Cache Directory Tagging Standard signature the cleaner requires, and
/// the engine's own state file, relative to `HOME`.
const CACHE_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";
const STATE_PATH: &str = ".cache/wisent-compute/disk-cleanup-state.json";

/// Directories the walk must cross, sized so the real pass outlasts
/// [`PASS_HEARTBEATS`] heartbeats; the assertion that it did is explicit.
const WORKLOAD_DIRECTORIES: usize = 24000;

/// The declared policy, every value inside the bounds
/// `targets::validation_disk` enforces and the deletion budget deliberately
/// small.
const SCHEMA_VERSION: u32 = 2;
const CHECK_INTERVAL_SECONDS: i64 = 60;
const LOW_FREE_GB: i64 = 1_000_000;
const TARGET_FREE_GB: i64 = 1_000_001;
const MAX_BYTES_PER_PASS: i64 = 1_073_741_824;
const MAX_SCAN_ITEMS: i64 = 200_000;
const MAX_PASS_SECONDS: i64 = 20;
const MIN_AGE_SECONDS: i64 = 86_400;
const ITEM_BUDGET: i64 = 5;
const ACTIVE_JOBS: i64 = 7;

/// The isolated host every test here measures. One fixture, not one per test:
/// the engine reads its store and home from the process environment.
struct Fixture {
    home: PathBuf,
    cache_root: PathBuf,
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(Fixture::new);

/// So a pass never meets another test's tree or another test's run lock.
async fn exclusive() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

impl Fixture {
    fn new() -> Self {
        let home = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/scratch/StubPurgeStado.RealProductAndJanitor")
            .join(format!("run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let storage = home.join("store");
        let cache_root = home.join("build-output");
        std::fs::create_dir_all(&storage).unwrap();
        std::fs::create_dir_all(&cache_root).unwrap();
        let hostname = String::from_utf8(Command::new("hostname").output().unwrap().stdout)
            .unwrap()
            .trim()
            .to_ascii_lowercase();
        let release_platform = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => "darwin-arm64",
            ("linux", "x86_64") => "linux-amd64",
            platform => panic!("unsupported janitor platform: {platform:?}"),
        };
        let policy = json!({
            "mode": "enforce",
            "check_interval_seconds": CHECK_INTERVAL_SECONDS,
            "low_free_gb": LOW_FREE_GB, "target_free_gb": TARGET_FREE_GB,
            "max_bytes_per_pass": MAX_BYTES_PER_PASS, "max_items_per_pass": ITEM_BUDGET,
            "max_scan_items": MAX_SCAN_ITEMS, "max_pass_seconds": MAX_PASS_SECONDS,
            "cleaners": {"build_caches": {"min_age_seconds": MIN_AGE_SECONDS, "root": cache_root}}
        });
        let registry = json!({
            "schema_version": SCHEMA_VERSION,
            "targets": [{
                "name": "janitor-runner", "kind": "local",
                "release_platform": release_platform, "ssh": "nobody@127.0.0.1",
                "hostnames": [hostname], "disk_cleanup": policy
            }],
            "coordinators": []
        });
        let document = serde_json::to_string_pretty(&registry).unwrap();
        std::fs::write(storage.join("registry.json"), document).unwrap();
        std::env::set_var("HOME", &home);
        std::env::set_var("STADO_CONFIG", home.join("absent-config.json"));
        std::env::set_var("WC_STORAGE_BACKEND", "local");
        std::env::set_var("WC_LOCAL_STORAGE_PATH", &storage);
        std::env::set_var("WC_PROVIDERS", "local");
        Self { home, cache_root }
    }

    /// Replace the cleaner root with `count` tagged cache directories, and
    /// retire the previous pass's state so this test's first pass is one the
    /// engine really runs. A fresh tree is below the engine's own
    /// `min_age_seconds`: the walk crosses it and deletes nothing, which is the
    /// 818-second `healthy_noop` shape exactly.
    fn workload(&self, count: usize, backdate: bool) -> usize {
        let _ = std::fs::remove_dir_all(self.home.join(".cache/wisent-compute"));
        let _ = std::fs::remove_dir_all(&self.cache_root);
        std::fs::create_dir_all(&self.cache_root).unwrap();
        let directories: Vec<PathBuf> = (0..count)
            .map(|index| {
                let directory = self.cache_root.join(format!("cache-{index}"));
                std::fs::create_dir(&directory).unwrap();
                std::fs::write(directory.join("CACHEDIR.TAG"), CACHE_TAG).unwrap();
                directory
            })
            .collect();
        if backdate {
            let touched = Command::new("/usr/bin/touch")
                .args(["-t", "202001010000"])
                .args(&directories)
                .status()
                .unwrap();
            assert!(touched.success(), "workload mtimes were not backdated");
        }
        count
    }

    fn surviving(&self) -> usize {
        std::fs::read_dir(&self.cache_root).unwrap().count()
    }

    /// The engine's own state file, which outlives every pass.
    fn persisted(&self) -> Value {
        let path = self.home.join(STATE_PATH);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("no persisted pass at {}: {error}", path.display()));
        serde_json::from_slice(&bytes).expect("the janitor state file is JSON")
    }
}

/// Start the product's real cleanup pass on the janitor's own thread.
fn spawn_real_janitor(reports: &JanitorReports) -> JanitorTask {
    reports.spawn_janitor(HEARTBEAT, |active_jobs| async move {
        let mut log = |_message: &str| {};
        run_cleanup_once(active_jobs, false, CleanupWriter::AgentTick, &mut log).await
    })
}

/// The FIRST completed pass, with the janitor stopped the moment it lands: the
/// declared `check_interval_seconds` is a minute, so every later pass in a
/// millisecond-scale test is an `interval_noop` that would overwrite a
/// measurement with a non-run.
async fn first_pass(reports: &JanitorReports, janitor: &JanitorTask) -> Value {
    let deadline = Instant::now() + PASS_DEADLINE;
    loop {
        if let Some(report) = reports.latest() {
            janitor.stop();
            return report;
        }
        assert!(Instant::now() < deadline, "no real pass completed");
        tokio::time::sleep(HEARTBEAT / 4).await;
    }
}

/// The constants must keep declaring the relation the fix relies on. Asserted
/// in a `const` block, so breaking it fails the BUILD rather than a test run:
/// if the heartbeat stops being strictly shorter than the staleness cutoff, a
/// punctual publisher goes stale and everything below defends the wrong thing.
#[test]
fn the_heartbeat_is_strictly_fresher_than_the_staleness_cutoff() {
    const {
        assert!(
            CAPACITY_HEARTBEAT_INTERVAL_S < CAPACITY_STALE_SECONDS,
            "the capacity heartbeat must be shorter than the staleness cutoff"
        );
        assert!(
            CAPACITY_STALE_SECONDS / CAPACITY_HEARTBEAT_INTERVAL_S >= 2,
            "the staleness cutoff must allow at least one missed heartbeat"
        );
    }
}

/// The defect, as an invariant, against the engine that caused it: a real pass
/// many heartbeats long must not stretch the gap between publications, and
/// `latest()` — the one line the tick calls — must never wait for a pass in
/// flight. Both are measured on the same real pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_cleanup_pass_does_not_delay_capacity_publication() {
    let _serial = exclusive().await;
    FIXTURE.workload(WORKLOAD_DIRECTORIES, false);
    let reports = JanitorReports::new();
    let janitor = spawn_real_janitor(&reports);

    // Tick as the agent's tick now does: read the latest COMPLETED report,
    // publish, move on. The loop ends on the first completed pass.
    let mut publications: Vec<Instant> = Vec::new();
    let mut worst_read = Duration::ZERO;
    let deadline = Instant::now() + PASS_DEADLINE;
    let report = loop {
        assert!(Instant::now() < deadline, "no real pass completed");
        reports.set_active_jobs(3);
        let read_at = Instant::now();
        let latest = reports.latest();
        worst_read = worst_read.max(read_at.elapsed());
        publications.push(Instant::now());
        if let Some(report) = latest {
            janitor.stop();
            break report;
        }
        tokio::time::sleep(HEARTBEAT).await;
    };

    let millis = report["duration_ms"]
        .as_i64()
        .unwrap_or_else(|| panic!("no duration_ms in {report:#}"));
    let pass = Duration::from_millis(u64::try_from(millis).unwrap());
    let stale = HEARTBEAT * (CAPACITY_STALE_SECONDS / CAPACITY_HEARTBEAT_INTERVAL_S) as u32;
    assert_eq!(report["writer"], "agent-tick", "real pass: {report:#}");
    assert!(
        pass >= HEARTBEAT * PASS_HEARTBEATS,
        "the real pass took {pass:?}, under {PASS_HEARTBEATS} heartbeats, so this proves nothing \
         about a pass outlasting the heartbeat; raise WORKLOAD_DIRECTORIES"
    );
    assert!(
        worst_read < HEARTBEAT,
        "reading the latest report waited {worst_read:?}, so a pass can block the tick"
    );
    let worst = publications
        .windows(2)
        .map(|pair| pair[1].duration_since(pair[0]))
        .max()
        .expect("at least two publications");
    assert!(
        worst < stale,
        "worst publication gap {worst:?} reached the staleness cutoff {stale:?}; a cleanup pass \
         is delaying capacity publication, which makes a healthy builder unselectable"
    );
    assert!(
        worst < pass,
        "worst publication gap {worst:?} is as long as the real pass {pass:?}, so the pass is \
         still on the publication's critical path"
    );
    assert_eq!(FIXTURE.surviving(), WORKLOAD_DIRECTORIES);
    assert_eq!(FIXTURE.persisted()["report"]["writer"], "agent-tick");
}

/// A completed real pass is what the tick publishes, and the job count the
/// tick recorded must reach the engine and come back in its report.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_completed_real_pass_becomes_the_report_the_tick_publishes() {
    let _serial = exclusive().await;
    let eligible = FIXTURE.workload((ITEM_BUDGET as usize) * 3, true);
    let reports = JanitorReports::new();
    reports.set_active_jobs(ACTIVE_JOBS);
    let janitor = spawn_real_janitor(&reports);
    let report = first_pass(&reports, &janitor).await;

    assert_eq!(report["writer"], "agent-tick", "real pass: {report:#}");
    assert_eq!(report["mode"], "enforce");
    assert_eq!(
        report["active_job_count"], ACTIVE_JOBS,
        "the pass must be told the job count the tick recorded"
    );
    let deleted = report["cleaners"]["build_caches"]["deleted_items"]
        .as_i64()
        .unwrap_or_else(|| panic!("the pass reached no cleaner: {report:#}"));
    assert!(
        deleted > 0 && deleted <= ITEM_BUDGET,
        "the real pass deleted {deleted} items against a declared budget of {ITEM_BUDGET}"
    );
    // Persisted, not merely reported: the tree shrank and the state file records it.
    assert!(
        FIXTURE.surviving() < eligible,
        "the real pass deleted nothing from the cleaner root"
    );
    let state = FIXTURE.persisted();
    assert_eq!(state["report"]["writer"], "agent-tick");
    assert_eq!(state["report"]["active_job_count"], ACTIVE_JOBS);
}
