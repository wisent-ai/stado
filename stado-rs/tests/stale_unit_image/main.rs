//! A managed unit whose live process is executing a file its unit no longer
//! names.
//!
//! The condition nothing in this fleet could see. `self_update::recycle_
//! replaced_units` cycles a unit only as a side effect of the invocation that
//! replaced the bytes: it joins the file names it just wrote onto the install
//! directory, reads each loaded unit's `argv` through `plutil`, and kickstarts
//! one only when `argv[0]` STRING-EQUALS a replaced path. There is no inode,
//! no mtime and no process-age comparison anywhere in it, a failed kickstart
//! only logs that the process keeps the old image, and nothing ever revisits a
//! process left behind. Staleness is detectable at replacement time and never
//! again.
//!
//! Measured on `lukasz-macbook` on 2026-09-02.
//! `com.wisent.compute.disk-cleanup.disk-cleanup` appended one JSON record per
//! pass to its log and failed on almost every one for thirteen days:
//! `policy:ValueError` 8,348 times, first at `2026-08-20T20:18:05Z`, last at
//! `2026-09-02T17:50:40Z`, then nothing — the first healthy pass came 55
//! seconds later, after the binary was replaced and the job restarted onto the
//! new image. The `--watch` process had been alive since 27 August, six days,
//! executing the image it started with while the file underneath it had been
//! replaced more than once.
//!
//! And it recurred the same evening, which is the strongest argument for this
//! check existing at all: at `2026-09-03T00:40:25Z` `~/.stado/bin/stado` became
//! inode 183456547 at 72,022,176 bytes (0.13.50), while pid 37842 —
//! `stado disk-cleanup --watch`, started 12:46:51 PDT — went on executing inode
//! 182274754 at 70,975,424 bytes, the size of the 0.13.47 release, with zero
//! links left. `recycle_replaced_units` had run and had not brought it forward,
//! and the janitor was HEALTHY on that image, because 0.13.47 still validates
//! today's registry. Staleness here is silent and harmless until the registry
//! moves, which is exactly what makes it dangerous.
//!
//! These tests defend that the condition is reported where an operator already
//! looks, that the row names the unit, the path and BOTH identities so nobody
//! has to re-derive them, that a deleted image is distinguished from a merely
//! different one, that a replacement still in flight is not a fault, and that
//! an identity which could not be read is reported as unknown rather than as
//! agreement — which is the exact defect this whole line of work exists to
//! remove.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, SystemTime};

use stado::deploy::service::{running_images, IMAGE_SETTLE_SECONDS};

const HOST: &str = "macbook-fake";
const STALE: &str = "stale-unit-image";
const UNREAD: &str = "unread-unit-image";

/// Set on the child this test drives, and never on the parent: `cargo test`
/// runs [`stale_image_probe_child`] like any other test and it must return
/// immediately there.
const HOLD: &str = "STADO_STALE_IMAGE_HOLD_SECONDS";

/// The unit the incident happened to was NOT in the registry's `services`
/// array — `com.wisent.compute.disk-cleanup.disk-cleanup` is installed by this
/// fleet and adopted by nothing — so the label this suite leans on is
/// undeclared too. A check that can only see declared units cannot see its own
/// motivating case.
const UNDECLARED: &str = "com.wisent.compute.disk-cleanup.probe-fake";

/// A declared unit, for the half of the enumeration that comes off the
/// document.
const DECLARED: &str = "com.wisent.probe-fake-declared";

/// A unit whose plist names no program at all.
const NO_PROGRAM: &str = "com.wisent.probe-fake-programless";

/// Not a test: the body of the controlled process the tests below drive.
///
/// It is a `#[test]` because the executable this suite needs is one it can
/// copy, and a locally built test binary is the only executable available to
/// it — macOS SIGKILLs a copy of a signed platform binary such as `/bin/sleep`
/// (exit 137, measured), so `/bin/sleep` cannot be the probe.
#[test]
fn stale_image_probe_child() {
    let Ok(seconds) = std::env::var(HOLD) else {
        return;
    };
    std::thread::sleep(Duration::from_secs(seconds.parse().unwrap_or(60)));
}

/// A child that is killed however the test leaves.
struct Probe {
    child: Child,
    /// The path it was executed from, which is also what its unit declares.
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
    /// Copy this test binary to `path`, start it, and wait until the kernel
    /// reports an image for it.
    ///
    /// `fs::copy` clones on APFS, so the copy costs no disk even though the
    /// binary is large.
    fn start(path: &Path) -> Self {
        std::fs::create_dir_all(path.parent().expect("probe has a parent")).expect("probe dir");
        let source = std::env::current_exe().expect("this test binary has a path");
        std::fs::copy(&source, path).expect("probe copy");
        let argv: Vec<String> = vec![
            path.display().to_string(),
            "--exact".to_string(),
            "stale_image_probe_child".to_string(),
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
        // By inode and not by path: `/var/folders/...` is a symlink to
        // `/private/var/folders/...` on macOS and `lsof` reports the resolved
        // spelling, which is the whole reason the check under test compares
        // identities rather than names.
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
        panic!("the probe never reported inode {inode} as its executing image at {path:?}");
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Unlink the file the probe is executing and put a different one at the
    /// same path — a replacement, exactly as an installer performs one.
    ///
    /// The new file's mtime is pushed back beyond [`IMAGE_SETTLE_SECONDS`]
    /// unless `settled` is false, which is how the mid-flight tolerance is
    /// exercised rather than waited out.
    fn replace_image(&self, settled: bool) {
        std::fs::remove_file(&self.path).expect("unlink the running image");
        std::fs::write(&self.path, b"#!/bin/sh\nexit 0\n").expect("write the replacement");
        if settled {
            backdate(&self.path, IMAGE_SETTLE_SECONDS + 60);
        }
    }
}

/// Push a file's mtime back by `seconds`.
fn backdate(path: &Path, seconds: i64) {
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open for retiming");
    let when = SystemTime::now() - Duration::from_secs(u64::try_from(seconds).expect("positive"));
    file.set_times(std::fs::FileTimes::new().set_modified(when))
        .expect("retime");
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

    /// The launchd directory the check scans, redirected at this test's HOME.
    fn agents(&self) -> PathBuf {
        self.root().join("home/Library/LaunchAgents")
    }

    /// Write a plist whose `ProgramArguments` are exactly `argv`, and return
    /// its path.
    ///
    /// Exactly `argv`, because launchd execs the vector verbatim and the check
    /// joins a process back to its unit on the whole vector: every stado unit
    /// on a host runs the same binary and the subcommand is the entire
    /// difference between them.
    fn plist(&self, label: &str, argv: &[String]) -> String {
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
        let path = self.agents().join(format!("{label}.plist"));
        std::fs::write(&path, body).expect("seed plist");
        path.display().to_string()
    }

    /// One always-on host declaring `services`.
    ///
    /// `is_self` names this machine's real hostname, which is the only way the
    /// command under test reads any image at all: which file a process is
    /// executing is answerable only on the machine holding that process, so
    /// the read is gated on the registry resolving this process's host to the
    /// target being checked.
    fn declare(&self, services: &[serde_json::Value], is_self: bool) {
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
                "services": services,
            }],
            "coordinators": [],
        });
        std::fs::write(
            self.root().join("storage/registry.json"),
            serde_json::to_string_pretty(&document).expect("registry document"),
        )
        .expect("seed registry");
    }

    fn stado(&self, args: &[&str]) -> Output {
        let root = self.root();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
        cmd.args(args)
            .env_clear()
            .env("HOME", root.join("home"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", root.join("storage"))
            .env("STADO_CONFIG", root.join("storage/no-such-config.json"));
        cmd.output().expect("stado binary runs")
    }

    /// Every finding of KIND that `registry doctor` reports.
    fn findings(&self, kind: &str) -> Vec<serde_json::Value> {
        let out = self.stado(&["registry", "doctor", "--json"]);
        let document: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|error| panic!("doctor emitted no JSON ({error}): {out:?}"));
        document
            .get("findings")
            .and_then(|value| value.as_array())
            .map(|rows| {
                rows.iter()
                    .filter(|row| row.get("finding").and_then(|v| v.as_str()) == Some(kind))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The rows of KIND that name `label`.
    ///
    /// Filtered by label rather than counted, because the machine running this
    /// test is a real host with real launchd directories and the check reads
    /// them: a fleet unit that genuinely is stale here is a true positive, not
    /// this test's business.
    fn about(&self, kind: &str, label: &str) -> Vec<String> {
        self.findings(kind)
            .iter()
            .filter_map(|row| row.get("detail").and_then(|v| v.as_str()))
            .filter(|detail| detail.contains(label))
            .map(str::to_string)
            .collect()
    }
}

/// This machine's hostname, by the same call `registry doctor` resolves its
/// own target with.
fn hostname() -> String {
    let out = Command::new("hostname")
        .output()
        .expect("hostname runs on this platform");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A `services[]` element as `stado service adopt` writes one.
fn unit(label: &str, path: &str) -> serde_json::Value {
    serde_json::json!({
        "name": label,
        "unit": "",
        "label": label,
        "path": path,
        "kind": "launchd",
        "managed_since": "2026-09-01T23:00:10.518396+00:00",
    })
}

/// The whole lifecycle on the shape the incident had: a fleet-installed unit
/// the registry does not declare, running, then replaced under itself.
///
/// Three phases in one test because they need the same live process, and the
/// process is the point: a fixture that never executed anything cannot show
/// that the image a running pid holds and the file at its path have come
/// apart.
#[test]
fn a_unit_whose_binary_was_replaced_under_it_is_reported_with_both_identities() {
    let harness = Harness::new();
    let probe = Probe::start(&harness.root().join("bin/probe"));
    harness.plist(UNDECLARED, &probe.argv);
    harness.declare(&[], true);

    // Phase one: the process is executing the file its unit names. Nothing to
    // report, and reporting anything here would make the check unusable.
    assert!(
        harness.about(STALE, UNDECLARED).is_empty(),
        "a unit running the file it declares must produce no row: {:?}",
        harness.about(STALE, UNDECLARED)
    );

    // Phase two: replaced this instant. The installer writes the bytes and
    // only afterwards cycles the units, so every managed process is
    // legitimately still on the old image for that window.
    probe.replace_image(false);
    assert!(
        harness.about(STALE, UNDECLARED).is_empty(),
        "a replacement younger than {IMAGE_SETTLE_SECONDS}s is mid-flight, not a fault: {:?}",
        harness.about(STALE, UNDECLARED)
    );

    // Phase three: the same replacement, older than the window the installer
    // needs. Nothing brought the process forward, and nothing will.
    backdate(&probe.path, IMAGE_SETTLE_SECONDS + 60);
    let rows = harness.about(STALE, UNDECLARED);
    assert_eq!(
        rows.len(),
        1,
        "exactly one row for one stale unit, got {rows:?}"
    );
    let detail = &rows[0];

    let running = running_images(&[probe.pid()])
        .expect("the image reader answers")
        .remove(&probe.pid())
        .expect("the probe is still running");
    let installed = std::fs::metadata(&probe.path).expect("the replacement is on disk");
    assert_eq!(
        running.links, 0,
        "the running image must have been unlinked for this to be the incident's shape"
    );
    for fact in [
        probe.path.display().to_string(),
        format!("{}", running.inode),
        format!("{} bytes", running.bytes),
        format!("{}", std::os::unix::fs::MetadataExt::ino(&installed)),
        UNDECLARED.to_string(),
    ] {
        assert!(
            detail.contains(&fact),
            "the row must name {fact} so nobody re-derives it: {detail}"
        );
    }
    assert!(
        detail.contains("unlinked"),
        "a deleted image must be named as deleted, not merely as different: {detail}"
    );

    let json = harness
        .findings(STALE)
        .into_iter()
        .find(|row| {
            row.get("detail")
                .and_then(|v| v.as_str())
                .is_some_and(|detail| detail.contains(UNDECLARED))
        })
        .expect("the row survives into --json");
    assert_eq!(json.get("subject").and_then(|v| v.as_str()), Some(HOST));
}

/// A running image that still has a name is a different operator problem from
/// one that has none, and the row says which.
#[test]
fn an_image_that_still_exists_elsewhere_is_replaced_and_not_unlinked() {
    let harness = Harness::new();
    let probe = Probe::start(&harness.root().join("bin/probe"));
    // A second name for the very bytes the probe is executing, so unlinking
    // the path it was started from leaves the image itself on disk.
    let kept = harness.root().join("bin/kept");
    std::fs::hard_link(&probe.path, &kept).expect("second link to the running image");
    let declared = harness.plist(DECLARED, &probe.argv);
    harness.declare(&[unit(DECLARED, &declared)], true);

    probe.replace_image(true);

    let rows = harness.about(STALE, DECLARED);
    assert_eq!(rows.len(), 1, "one declared unit, one row, got {rows:?}");
    let detail = &rows[0];
    assert!(
        !detail.contains("unlinked"),
        "an image with a name left is not the unlinked case: {detail}"
    );
    assert!(
        detail.contains("is not the file its unit declares"),
        "the row must still say the process is on the wrong file: {detail}"
    );
    let running = running_images(&[probe.pid()])
        .expect("the image reader answers")
        .remove(&probe.pid())
        .expect("the probe is still running");
    assert_eq!(
        running.links, 1,
        "the surviving hard link is what makes this the replaced case"
    );
    assert!(
        detail.contains(&format!("{}", running.inode)),
        "the row must name the running inode: {detail}"
    );
    drop(kept);
}

/// A unit whose declaration cannot be read is reported as unknown.
///
/// The rule `registry doctor` already applies to unit files it cannot open:
/// "nothing was read" and "nothing is wrong" are different facts, and
/// rendering the first as the second is the defect this whole check exists to
/// remove.
#[test]
fn a_declaration_that_names_no_program_is_unknown_and_never_clean() {
    let harness = Harness::new();
    let declared = harness.plist(NO_PROGRAM, &[]);
    harness.declare(&[unit(NO_PROGRAM, &declared)], true);

    let rows = harness.about(UNREAD, NO_PROGRAM);
    assert_eq!(
        rows.len(),
        1,
        "a plist that declares no program is one unread row, got {rows:?}"
    );
    assert!(
        rows[0].contains("is unknown here and is NOT reported as agreement"),
        "the row must refuse to read as a pass: {}",
        rows[0]
    );
    assert!(
        harness.about(STALE, NO_PROGRAM).is_empty(),
        "an unread unit is never also reported as stale"
    );
}

/// Another host's units are reported unread rather than omitted.
///
/// Which image a process is executing is readable only on the machine holding
/// that process. `registry doctor` answers for the whole fleet, so every host
/// it is not running on has to say so — a remote host silently skipped would
/// be an unmeasured state rendering as a passing one, wearing this check's
/// name.
#[test]
fn a_unit_on_another_host_is_unread_rather_than_clean() {
    let harness = Harness::new();
    let declared = harness.plist(DECLARED, &["/does/not/matter".to_string()]);
    harness.declare(&[unit(DECLARED, &declared)], false);

    let rows = harness.about(UNREAD, HOST);
    assert_eq!(
        rows.len(),
        1,
        "one row for the whole unread host, not one per unit, got {rows:?}"
    );
    assert!(
        rows[0].contains("readable only on the machine holding that process"),
        "the row must say why it could not be measured: {}",
        rows[0]
    );
    assert!(
        harness.findings(STALE).is_empty(),
        "no image on another host can be judged stale from here"
    );
}
