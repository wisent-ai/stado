//! The post-restart half: what the second read found, and what the command
//! does with it.
//!
//! The first case is the whole verb end to end against a launchd unit this
//! test loads into its own login domain — the coverage this area used to
//! state as a gap. The rest decide the branches a real restart on this
//! machine cannot produce, from identities read off real files this test
//! wrote: `Unchanged` is a process respawning onto an inode it had already
//! left, which is launchd re-execing a cached path (pid 49727, 2026-09-03)
//! and not something a test can ask launchd to do.

use std::path::Path;

use serde_json::Value;

use stado::cli::service_refresh_image::{refresh_outcome, RefreshOutcome};
use stado::deploy::service::{
    installed_image, ImageIdentity, ImageState, UnitImageObservation, IMAGE_SETTLE_SECONDS,
};

use crate::host::{digest, local_host_name, said, Host};
use crate::unit::{current_exe, label, LoadedUnit};

/// A stale unit is restarted and lands on the file it declares.
///
/// Every fact here is measured: the unit is loaded by `launchctl bootstrap`,
/// the image is replaced on disk, the command really kickstarts the job, and
/// the process that comes back is checked against the declared file's inode
/// and against a sha256 this test computed of the bytes it wrote.
#[test]
fn a_stale_unit_is_restarted_onto_the_file_it_declares() {
    let host = Host::new();
    let mut unit = LoadedUnit::start(&host, "landed");
    let was = unit.image();
    let replacement = unit.replace_image();
    unit.backdate(IMAGE_SETTLE_SECONDS + 60);
    let (installed, _written) =
        installed_image(&unit.program).expect("the replacement is readable on disk");
    assert!(
        !was.is_same_file(&installed),
        "the process and its declared file must be different files before the refresh"
    );

    let output = host.stado(&["service", "refresh-image", &unit.label, "--json"]);
    let said = said(&output);
    assert!(
        output.status.success(),
        "a restart that landed is the one success this command has: {said}"
    );
    let report: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("refresh-image printed no JSON document ({error}): {said}"));

    assert_eq!(
        report["restarted"].as_str(),
        Some(unit.qualified().as_str()),
        "the command must restart the unit in the domain it is loaded in: {report:#?}"
    );
    assert_eq!(
        report["before"]["running"]["inode"].as_u64(),
        Some(was.inode),
        "the report must carry the image the process was on: {report:#?}"
    );
    assert_eq!(
        report["after"]["agrees"],
        Value::Bool(true),
        "the second read must find the new process on the declared file: {report:#?}"
    );
    assert_eq!(
        report["after"]["running"]["inode"].as_u64(),
        Some(installed.inode),
        "the new process must be executing the inode this test wrote: {report:#?}"
    );
    let landed = report["after"]["pid"]
        .as_u64()
        .expect("the second read names a pid");
    assert_ne!(
        landed,
        u64::from(unit.pid),
        "a restart that reported the pid it kicked would be reporting nothing"
    );
    assert_eq!(
        unit.live_pid().map(u64::from),
        Some(landed),
        "launchd must hold the pid the report named"
    );
    let running_path = report["after"]["running"]["path"]
        .as_str()
        .expect("the second read names the image it found");
    assert_eq!(
        digest(Path::new(running_path)),
        replacement,
        "the bytes the new process executes must be the replacement this test wrote"
    );
    assert_ne!(replacement, unit.digest, "and not the bytes it started on");

    // The unit this case restarted is removed again, and the removal is read
    // off launchd rather than assumed.
    let removed = unit.remove();
    assert!(
        removed.contains("Could not find service") && removed.contains(&unit.label),
        "launchd must report the label this test loaded as gone: {removed}"
    );
    assert_eq!(
        unit.live_pid(),
        None,
        "no process may be left holding the label"
    );
}

/// Real files, and the identity this machine reports for each.
///
/// No inode, device, size or link count below is invented: every one is read
/// off a file this test wrote, which is what keeps the branch decisions honest
/// even where a real launchd restart cannot reach them.
struct Files {
    _dir: tempfile::TempDir,
    images: Vec<ImageIdentity>,
    written: Vec<i64>,
}

impl Files {
    fn new(count: usize) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("stado-refresh-identities-")
            .tempdir()
            .expect("create the identity tempdir");
        let mut images = Vec::new();
        let mut written = Vec::new();
        for index in 0..count {
            let path = dir.path().join(format!("image-{index}"));
            std::fs::copy(current_exe(), &path).expect("place a real executable");
            std::fs::write(dir.path().join(format!("mark-{index}")), format!("{index}"))
                .expect("mark the file apart");
            let (image, epoch) = installed_image(&path).expect("read the identity of a real file");
            images.push(image);
            written.push(epoch);
        }
        Self {
            _dir: dir,
            images,
            written,
        }
    }

    fn image(&self, index: usize) -> ImageIdentity {
        self.images[index].clone()
    }

    /// The observation the post-restart read produces, on real identities.
    fn observed(
        &self,
        running: Option<ImageIdentity>,
        installed: Option<ImageIdentity>,
    ) -> UnitImageObservation {
        let declared = installed.clone().unwrap_or_else(|| self.image(0));
        UnitImageObservation {
            host: local_host_name(),
            unit: label("verdict"),
            unit_path: format!("{declared}.plist", declared = declared.path),
            program: declared.path.clone(),
            pid: Some(std::process::id()),
            // Assembled from files rather than from a live unit, so no process
            // age is claimed. The installed age is the real one.
            process_age_seconds: None,
            installed_age_seconds: Some(chrono::Utc::now().timestamp() - self.written[0]),
            running,
            installed,
            state: None,
        }
    }
}

/// A restart that landed is the only success.
#[test]
fn only_a_process_on_the_declared_file_counts_as_fixed() {
    let files = Files::new(1);
    let landed = files.observed(Some(files.image(0)), Some(files.image(0)));
    let outcome = refresh_outcome(&files.image(0), Some(&landed));
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
    let files = Files::new(2);
    let unchanged = files.observed(Some(files.image(0)), Some(files.image(1)));
    let outcome = refresh_outcome(&files.image(0), Some(&unchanged));
    assert_eq!(outcome, RefreshOutcome::Unchanged);
    assert!(
        !outcome.succeeded(),
        "issuing a restart is not evidence that anything changed"
    );
}

/// Landing on a third file is its own answer, kept apart from both.
#[test]
fn landing_on_a_third_file_is_neither_fixed_nor_unchanged() {
    let files = Files::new(3);
    let elsewhere = files.observed(Some(files.image(2)), Some(files.image(1)));
    assert_eq!(
        refresh_outcome(&files.image(0), Some(&elsewhere)),
        RefreshOutcome::StillWrong
    );
}

/// A unit that did not come back, and one whose result could not be read, are
/// two different failures and neither is a pass.
#[test]
fn a_unit_that_did_not_come_back_and_one_that_could_not_be_read_both_fail() {
    let files = Files::new(2);
    assert_eq!(
        refresh_outcome(&files.image(0), None),
        RefreshOutcome::NotRunning
    );
    let unread = files.observed(None, Some(files.image(1)));
    assert_eq!(
        refresh_outcome(&files.image(0), Some(&unread)),
        RefreshOutcome::Unread
    );
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
    let files = Files::new(2);
    let stale = UnitImageObservation {
        state: Some(ImageState::Unlinked {
            running: files.image(0),
            installed: files.image(1),
        }),
        ..files.observed(Some(files.image(0)), Some(files.image(1)))
    };
    let finding = stale.finding().expect("a stale observation is a finding");
    assert_eq!(finding.kind(), "stale-unit-image");
    assert_eq!(finding.unit, label("verdict"));
    assert_eq!(finding.host, local_host_name());
    assert_eq!(stale.agrees(), Some(false));

    let clean = files.observed(Some(files.image(1)), Some(files.image(1)));
    assert!(
        clean.finding().is_none(),
        "a unit on its declared file is no finding"
    );
    assert_eq!(clean.agrees(), Some(true));

    let unread = files.observed(None, Some(files.image(1)));
    assert_eq!(
        unread.agrees(),
        None,
        "an unread identity must never answer true"
    );
}
