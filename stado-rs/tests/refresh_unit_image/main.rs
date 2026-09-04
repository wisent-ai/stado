//! `stado service refresh-image` — the verb behind the `stale-unit-image` row.
//!
//! `registry doctor` reports a unit whose live process is executing a file the
//! unit no longer names, and the row ends "Restarting the unit is what puts it
//! on the installed file, and nothing does that on its own". Until this
//! command that sentence instructed an operator to perform an action the
//! product did not offer as a checked operation.
//!
//! What these tests defend is mostly the refusals, because the refusals are
//! where the damage is. A remediation that restarts whatever it is pointed at
//! is a restart button, and on this fleet a restart button has already turned
//! a degraded host into a down one. So: a unit that is not stale is refused
//! and the refusal names the identity that was read; a replacement still
//! inside the settle window is refused as an installer mid-flight; a unit
//! whose identity could not be read is refused, because an unread state is no
//! more a reason to act than it is a reason to pass; and a machine no registry
//! target names is refused before anything is looked at.
//!
//! And the post-restart verdict, which is the other half of the discipline. On
//! 2026-09-03 pid 49727 — `com.wisent.compute.agent.lukasz-macbook` —
//! respawned under `KeepAlive` straight back onto the same unlinked inode
//! 182274754 it had just left. launchd re-execs the declared path and the path
//! was never the problem, so "a restart was issued" is not evidence that
//! anything changed.
//!
//! HONEST GAP, stated rather than papered over: neither post-restart branch is
//! exercised here against a real launchd restart. Reaching `OnDeclaredFile` or
//! `Unchanged` end to end needs a unit launchd actually holds, and loading one
//! means `launchctl bootstrap` against this machine's real launchd — which is
//! exactly the mutation this work is under instruction not to perform. So
//! [`refresh_outcome`] is pure and public, both branches are decided by tested
//! logic, and the end-to-end coverage stops at the last refusal before the
//! kickstart. That is the coverage claimed and no more.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, SystemTime};

use stado::cli::service_refresh_image::{refresh_outcome, RefreshOutcome};
use stado::deploy::service::{running_images, ImageIdentity, UnitImageObservation};
use stado::deploy::service::{ImageState, IMAGE_SETTLE_SECONDS};

const HOST: &str = "macbook-fake";
const HOLD: &str = "STADO_REFRESH_IMAGE_HOLD_SECONDS";

/// The label these tests point the command at. Fleet-prefixed, so the check's
/// directory enumeration picks it up, and unmistakably fake so nothing here
/// can address a unit this machine really holds.
const UNIT: &str = "com.wisent.compute.refresh-fake.probe";

/// Not a test: the body of the controlled process these tests drive.
///
/// A locally built binary, because macOS SIGKILLs a copy of a signed platform
/// binary — `/bin/sleep` copied out of place exits 137, measured.
#[test]
fn refresh_image_probe_child() {
    let Ok(seconds) = std::env::var(HOLD) else {
        return;
    };
    std::thread::sleep(Duration::from_secs(seconds.parse().unwrap_or(60)));
}

struct Probe {
    child: Child,
    path: PathBuf,
    argv: Vec<String>,
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

impl Probe {
    fn start(path: &Path) -> Self {
        std::fs::create_dir_all(path.parent().expect("probe has a parent")).expect("probe dir");
        let source = std::env::current_exe().expect("this test binary has a path");
        std::fs::copy(&source, path).expect("probe copy");
        let argv: Vec<String> = vec![
            path.display().to_string(),
            "--exact".to_string(),
            "refresh_image_probe_child".to_string(),
        ];
        let child = Command::new(path)
            .args(&argv[1..])
            .env(HOLD, "120")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("probe starts");
        let probe = Self {
            child,
            path: path.to_path_buf(),
            argv,
        };
        // By inode, not by path: `/var/folders/...` is a symlink to
        // `/private/var/folders/...` and `lsof` reports the resolved spelling.
        let inode = std::fs::metadata(path)
            .map(|metadata| std::os::unix::fs::MetadataExt::ino(&metadata))
            .expect("the probe copy is on disk");
        for _ in 0..100 {
            if let Ok(images) = running_images(&[probe.pid()]) {
                if images
                    .get(&probe.pid())
                    .is_some_and(|image| image.inode == inode)
                {
                    return probe;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("the probe never reported inode {inode} as its executing image");
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Unlink the running image and put a different file at the same path.
    fn replace_image(&self, settled: bool) {
        std::fs::remove_file(&self.path).expect("unlink the running image");
        std::fs::write(&self.path, b"#!/bin/sh\nexit 0\n").expect("write the replacement");
        if settled {
            let file = std::fs::File::options()
                .write(true)
                .open(&self.path)
                .expect("open for retiming");
            let when = SystemTime::now()
                - Duration::from_secs(u64::try_from(IMAGE_SETTLE_SECONDS + 60).expect("positive"));
            file.set_times(std::fs::FileTimes::new().set_modified(when))
                .expect("retime");
        }
    }
}

struct Harness {
    dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let harness = Self {
            dir: tempfile::tempdir().expect("temp root"),
        };
        for sub in [
            "storage",
            "storage/host_health",
            "home",
            "home/Library/LaunchAgents",
            "bin",
        ] {
            std::fs::create_dir_all(harness.root().join(sub)).expect("temp subdirectory");
        }
        harness
    }

    fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    fn plist(&self, label: &str, argv: &[String]) {
        let entries = argv
            .iter()
            .map(|word| format!("    <string>{word}</string>"))
            .collect::<Vec<String>>()
            .join("\n");
        let arguments = if argv.is_empty() {
            String::new()
        } else {
            format!("  <key>ProgramArguments</key>\n  <array>\n{entries}\n  </array>\n")
        };
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>KeepAlive</key><true/>
{arguments}</dict>
</plist>
"#
        );
        std::fs::write(
            self.root()
                .join("home/Library/LaunchAgents")
                .join(format!("{label}.plist")),
            body,
        )
        .expect("seed plist");
    }

    /// One always-on host. `is_self` names this machine's real hostname, which
    /// is what makes the command willing to read any image at all.
    fn declare(&self, is_self: bool) {
        let hostnames = if is_self {
            vec![
                hostname(),
                hostname().trim_end_matches(".local").to_string(),
            ]
        } else {
            vec![format!("{HOST}.local")]
        };
        let document = serde_json::json!({
            "schema_version": 2,
            "targets": [{
                "name": HOST,
                "kind": "local",
                "ssh": "lukasz@10.9.9.30",
                "release_platform": "darwin-arm64",
                "hostnames": hostnames,
                "role": "always-on",
                "host_heuristic": "always-on",
                "managed_versions": {},
                "services": [],
            }],
            "coordinators": [],
        });
        std::fs::write(
            self.root().join("storage/registry.json"),
            serde_json::to_string_pretty(&document).expect("registry document"),
        )
        .expect("seed registry");
    }

    fn refresh(&self, name: &str) -> Output {
        let root = self.root();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
        cmd.args(["service", "refresh-image", name])
            .env_clear()
            .env("HOME", root.join("home"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", root.join("storage"))
            .env("STADO_CONFIG", root.join("storage/no-such-config.json"));
        cmd.output().expect("stado binary runs")
    }
}

fn hostname() -> String {
    let out = Command::new("hostname")
        .output()
        .expect("hostname runs on this platform");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Everything the command wrote, so an assertion does not have to guess which
/// stream carried the refusal.
fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A unit executing the file it declares is left alone, and the refusal names
/// what was found.
///
/// The load-bearing assertion is the last one: the process is still alive with
/// the pid it had. A refusal that restarted first would be no refusal at all.
#[test]
fn a_unit_that_is_not_stale_is_refused_and_left_running() {
    let harness = Harness::new();
    let mut probe = Probe::start(&harness.root().join("bin/probe"));
    harness.plist(UNIT, &probe.argv);
    harness.declare(true);

    let out = harness.refresh(UNIT);
    let said = said(&out);
    assert!(
        !out.status.success(),
        "a healthy unit must exit non-zero to refuse: {said}"
    );
    assert!(
        said.contains("is not stale and was not restarted"),
        "the refusal must say it refused: {said}"
    );
    let running = running_images(&[probe.pid()])
        .expect("the image reader answers")
        .remove(&probe.pid())
        .expect("the probe is still running");
    assert!(
        said.contains(&format!("{}", running.inode)),
        "the refusal must name the identity it found: {said}"
    );
    assert!(
        probe.alive(),
        "the probe must still be running: a refusal that restarts first is a restart button"
    );
}

/// A replacement younger than the settle window is an installer mid-flight.
#[test]
fn a_replacement_still_in_flight_is_refused() {
    let harness = Harness::new();
    let mut probe = Probe::start(&harness.root().join("bin/probe"));
    harness.plist(UNIT, &probe.argv);
    harness.declare(true);
    probe.replace_image(false);

    let out = harness.refresh(UNIT);
    let said = said(&out);
    assert!(
        !out.status.success(),
        "mid-flight must exit non-zero: {said}"
    );
    assert!(
        said.contains(&format!("less than {IMAGE_SETTLE_SECONDS}s ago")),
        "the refusal must say the replacement is too young: {said}"
    );
    assert!(probe.alive(), "nothing may be restarted mid-flight");
}

/// An identity that could not be read is not a reason to act.
#[test]
fn an_unread_unit_is_refused_rather_than_restarted() {
    let harness = Harness::new();
    harness.plist(UNIT, &[]);
    harness.declare(true);

    let out = harness.refresh(UNIT);
    let said = said(&out);
    assert!(!out.status.success(), "unread must exit non-zero: {said}");
    assert!(
        said.contains("whether it is stale is unknown")
            || said.contains("holds no launchd unit named"),
        "the refusal must name the unknown rather than acting on it: {said}"
    );
    assert!(
        !said.contains("restarted "),
        "nothing may be restarted on an unread identity: {said}"
    );
}

/// A label this machine holds no running unit for is refused with the reason.
#[test]
fn a_unit_with_no_live_process_is_refused_with_the_reason() {
    let harness = Harness::new();
    harness.declare(true);

    let out = harness.refresh(UNIT);
    let said = said(&out);
    assert!(
        !out.status.success(),
        "an unknown unit must exit non-zero: {said}"
    );
    assert!(
        said.contains("holds no launchd unit named"),
        "the refusal must say what it looked for: {said}"
    );
    assert!(
        said.contains("a job that is not running holds no image"),
        "the refusal must say why that is not a fault: {said}"
    );
}

/// A machine no registry target names is refused before anything is read.
#[test]
fn a_machine_no_target_names_is_refused_before_it_looks() {
    let harness = Harness::new();
    harness.declare(false);

    let out = harness.refresh(UNIT);
    let said = said(&out);
    assert!(!out.status.success(), "must exit non-zero: {said}");
    assert!(
        said.contains("readable only on the machine holding that process"),
        "the refusal must say the read is local: {said}"
    );
}

/// The identity in the shape the observation carries it.
fn identity(inode: u64, links: u64) -> ImageIdentity {
    ImageIdentity {
        path: "/opt/probe".to_string(),
        device: 0x0100_000d,
        inode,
        bytes: 1024,
        links,
    }
}

/// An observation as the post-restart read would produce one.
fn after(running: Option<ImageIdentity>, installed: Option<ImageIdentity>) -> UnitImageObservation {
    UnitImageObservation {
        host: HOST.to_string(),
        unit: UNIT.to_string(),
        unit_path: "/opt/probe.plist".to_string(),
        program: "/opt/probe".to_string(),
        pid: Some(4242),
        process_age_seconds: Some(2),
        installed_age_seconds: Some(IMAGE_SETTLE_SECONDS + 60),
        running,
        installed,
        state: None,
    }
}

/// A restart that landed is the only success.
#[test]
fn only_a_process_on_the_declared_file_counts_as_fixed() {
    let was = identity(111, 0);
    let landed = after(Some(identity(222, 1)), Some(identity(222, 1)));
    let outcome = refresh_outcome(&was, Some(&landed));
    assert_eq!(outcome, RefreshOutcome::OnDeclaredFile);
    assert!(outcome.succeeded());
}

/// A respawn onto the same image is a failure, not a caveat.
///
/// This is pid 49727's behaviour on 2026-09-03: `KeepAlive` brought the unit
/// back on the very inode it had just left, because launchd re-execs the
/// declared path and the path was never the problem.
#[test]
fn a_respawn_onto_the_same_image_is_not_success() {
    let was = identity(182_274_754, 0);
    let unchanged = after(
        Some(identity(182_274_754, 0)),
        Some(identity(183_456_547, 1)),
    );
    let outcome = refresh_outcome(&was, Some(&unchanged));
    assert_eq!(outcome, RefreshOutcome::Unchanged);
    assert!(
        !outcome.succeeded(),
        "issuing a restart is not evidence that anything changed"
    );
}

/// Landing on a third file is its own answer, kept apart from both.
#[test]
fn landing_on_a_third_file_is_neither_fixed_nor_unchanged() {
    let was = identity(111, 0);
    let elsewhere = after(Some(identity(333, 1)), Some(identity(222, 1)));
    assert_eq!(
        refresh_outcome(&was, Some(&elsewhere)),
        RefreshOutcome::StillWrong
    );
}

/// A unit that did not come back, and one whose result could not be read, are
/// two different failures and neither is a pass.
#[test]
fn a_unit_that_did_not_come_back_and_one_that_could_not_be_read_both_fail() {
    let was = identity(111, 0);
    assert_eq!(refresh_outcome(&was, None), RefreshOutcome::NotRunning);
    let unread = after(None, Some(identity(222, 1)));
    assert_eq!(refresh_outcome(&was, Some(&unread)), RefreshOutcome::Unread);
    for outcome in [
        RefreshOutcome::NotRunning,
        RefreshOutcome::Unread,
        RefreshOutcome::Unchanged,
        RefreshOutcome::StillWrong,
    ] {
        assert!(!outcome.succeeded(), "{outcome:?} must not read as success");
    }
}

/// The finding the doctor prints and the state this command acts on come from
/// one pass, so they cannot drift.
#[test]
fn the_command_and_the_doctor_read_one_observation() {
    let stale = UnitImageObservation {
        state: Some(ImageState::Unlinked {
            running: identity(111, 0),
            installed: identity(222, 1),
        }),
        ..after(Some(identity(111, 0)), Some(identity(222, 1)))
    };
    let finding = stale.finding().expect("a stale observation is a finding");
    assert_eq!(finding.kind(), "stale-unit-image");
    assert_eq!(finding.unit, UNIT);
    assert_eq!(stale.agrees(), Some(false));

    let clean = after(Some(identity(222, 1)), Some(identity(222, 1)));
    assert!(
        clean.finding().is_none(),
        "a unit on its declared file is no finding"
    );
    assert_eq!(clean.agrees(), Some(true));

    let unread = after(None, Some(identity(222, 1)));
    assert_eq!(
        unread.agrees(),
        None,
        "an unread identity must never answer true"
    );
}
