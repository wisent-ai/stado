//! A periodic janitor states when its next pass will land, keeps it, and
//! `host gates` judges it by that statement rather than by a window; its
//! kernel lock, live or retired, keeps a pass out until the kernel releases it.

use std::fs::{self, FileTimes, OpenOptions};
use std::io::{BufRead, BufReader, Lines};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::native::Native;

/// `stado serve --disk-cleanup --health-interval-seconds N` in the isolated
/// home, its reports read line by line as each pass prints one.
struct Watch {
    child: Child,
    reports: Lines<BufReader<ChildStdout>>,
}

impl Watch {
    fn start(native: &Native, interval: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args([
                "serve",
                "--disk-cleanup",
                "--health-interval-seconds",
                interval,
            ])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH is set"))
            .env("HOME", &native.home)
            .env("STADO_CONFIG", native.home.join(".stado/config.json"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start the watch");
        let reports = BufReader::new(child.stdout.take().unwrap()).lines();
        Self { child, reports }
    }

    fn next_report(&mut self) -> Value {
        let line = self
            .reports
            .next()
            .expect("the watch printed a pass")
            .unwrap();
        serde_json::from_str(&line).expect("a pass report is JSON")
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn instant(value: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value.as_str().expect("a timestamp"))
        .expect("an RFC 3339 timestamp")
        .with_timezone(&Utc)
}

fn state(native: &Native) -> Value {
    let body = fs::read_to_string(native.state_dir().join("disk-cleanup-state.json")).unwrap();
    serde_json::from_str(&body).unwrap()
}
/// `host gates` exits non-zero while any blocker stands, and this isolated
/// host has no capacity publication, so its verdict is read from stdout.
fn gates(native: &Native) -> Value {
    let output = native.run(&["host", "gates", "example-cleanup-host", "--json"]);
    serde_json::from_slice(&output.stdout).expect("host gates printed JSON")
}

#[test]
fn the_watch_reads_the_volume_at_the_hosts_health_period() {
    let native = Native::new("watch-period-refusals");
    let serve = native.run(&["serve", "--disk-cleanup"]);
    assert_eq!(serve.status.code(), Some(2), "{serve:?}");
    assert!(
        String::from_utf8_lossy(&serve.stderr).contains("--health-interval-seconds"),
        "{serve:?}"
    );
}

#[test]
fn a_watch_keeps_the_pass_it_promised_and_gates_read_the_promise() {
    let native = Native::new("watch-promise");
    let mut watch = Watch::start(&native, "2");
    let first = watch.next_report();
    assert_eq!(first["writer"], "disk-cleanup-cli", "{first}");
    assert_eq!(first["every_seconds"], 2, "{first}");
    // The second pass is the first one this process can measure its
    // lateness from: the time between the first pass's end and its own start
    // beyond the period, spent writing the state file and printing.
    let second = watch.next_report();
    let promised = state(&native)["promises"]["disk-cleanup-cli"].clone();
    native.observe("measured promise", promised.clone());
    assert_eq!(promised["every_seconds"], 2, "{promised}");
    assert_eq!(promised["pid"], second["writer_pid"], "{promised}");
    let part = |key: &str| {
        promised[key]
            .as_f64()
            .unwrap_or_else(|| panic!("{key} in {promised}"))
    };
    assert!(part("longest_lateness_seconds") > 0.0, "{promised}");
    assert!(
        part("longest_pass_seconds") >= second["duration_ms"].as_f64().unwrap() / 1000.0,
        "{promised}"
    );
    let expected =
        part("finished_at") + 2.0 + part("longest_pass_seconds") + part("longest_lateness_seconds");
    assert!((part("next_pass_by") - expected).abs() < 1e-6, "{promised}");

    let report = native.json(&["space", "report", "example-cleanup-host", "--json"]);
    let cleanup_state = &report["cleanup_state"];
    native.observe("space report", cleanup_state.clone());
    assert_eq!(
        cleanup_state["promised_by"], "disk-cleanup-cli",
        "{cleanup_state}"
    );
    assert_eq!(cleanup_state["prevented"], false, "{cleanup_state}");
    assert!(
        instant(&cleanup_state["next_pass_by"]) > instant(&cleanup_state["last_pass_at"]),
        "{cleanup_state}"
    );
    let kept = gates(&native);
    native.observe("gates while the promise holds", kept.clone());
    assert_eq!(kept["disk"]["cleanup_stalled"], false, "{kept}");
    drop(watch);

    // A janitor from before the promise states none: the state it leaves is
    // this file without `promises`, and nothing then promises a pass.
    let mut legacy = state(&native);
    legacy.as_object_mut().unwrap().remove("promises");
    fs::write(
        native.state_dir().join("disk-cleanup-state.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    let unpromised = gates(&native);
    native.observe("gates with no promise stated", unpromised.clone());
    assert_eq!(unpromised["disk"]["cleanup_stalled"], true, "{unpromised}");
}
#[test]
fn an_aged_live_kernel_lock_is_not_replaced_and_release_allows_cleanup() {
    let native = Native::new("live-lock");
    let candidate = native.home.join("work/target");
    native.cache(&candidate);
    native.fill_with_user_data();
    let state_dir = native.state_dir();
    fs::create_dir_all(&state_dir).unwrap();
    let path = state_dir.join("disk-cleanup.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    lock.set_times(FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
        .unwrap();
    let before = lock.metadata().unwrap();
    let report = native.cleanup();
    let after = path.metadata().unwrap();
    native.observe(
        "held kernel lock",
        json!({
            "before": {"device": before.dev(), "inode": before.ino()},
            "after": {"device": after.dev(), "inode": after.ino()},
            "candidate_present": candidate.is_dir(),
        }),
    );
    assert_eq!(
        (before.dev(), before.ino()),
        (after.dev(), after.ino()),
        "a live kernel lock was replaced because of its age"
    );
    assert_eq!(report["lock_busy"], true);
    assert!(
        candidate.is_dir(),
        "cleanup deleted while another process held its lock"
    );
    drop(lock);
    native.cleanup();
    assert!(
        !candidate.exists(),
        "kernel release did not permit reclamation"
    );
}

#[test]
fn an_already_retired_locked_inode_remains_protected_until_kernel_release() {
    let native = Native::new("retired-lock");
    let candidate = native.home.join("work/target");
    native.cache(&candidate);
    native.fill_with_user_data();
    let state_dir = native.state_dir();
    fs::create_dir_all(&state_dir).unwrap();
    let path = state_dir.join("disk-cleanup.lock.retired.regression");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    let held = native.cleanup();
    assert_eq!(held["outcome"], "lock_recovery_report_only");
    assert!(
        candidate.is_dir(),
        "cleanup bypassed a held predecessor inode"
    );
    drop(lock);
    native.cleanup();
    assert!(
        !candidate.exists(),
        "an unlocked predecessor still prevented reclamation"
    );
    assert!(
        !path.exists(),
        "the unlocked predecessor remained in persisted state"
    );
}
