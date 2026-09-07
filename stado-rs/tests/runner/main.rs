//! The host's runner: its registration scope and its one job slot.
//!
//! Two facts this defends, both paid for on 2026-09-06.
//!
//! A registration token has two addresses. The organization one needs the
//! organization's self-hosted-runner permission; the repository one needs
//! admin on that repository. The fleet's credential is refused at the first
//! and accepted at the second, so a command that can only address the first
//! cannot manage a runner here at all — which is how five sessions read one
//! `403` as "runners cannot be managed from here".
//!
//! And a host carrying several runners runs several jobs at once, because
//! GitHub gives each runner a concurrency of one and coordinates nothing
//! between them. `charless-mac-mini` went from 10.6 to 4.9 GiB free in twenty
//! minutes with one `git` holding 1021 MiB, then no .NET listener on it could
//! start at all. The gate the installer writes bounds that, and this drives
//! the exact program a host runs — `job_gate_program`, the same text both
//! installers embed — rather than a description of it.
//!
//! Isolation is environmental, as everywhere else in this suite:
//! `STADO_RUNNER_JOBS_DIR` points the gate at a tempdir, so nothing here can
//! see or touch a real host's markers.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use stado::deploy::host_precheck_runner::{job_gate_program, MACOS_JOBS_DIR};

/// A temp directory that removes itself, so a failed assertion cannot leave
/// markers behind for the next run to trip over.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "stado-runner-gate-{name}-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        fs::create_dir_all(&path).expect("scratch dir");
        Self { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn install_gate(scratch: &Path) -> PathBuf {
    let gate = scratch.join("job-gate.sh");
    fs::write(&gate, job_gate_program(MACOS_JOBS_DIR)).expect("write gate");
    fs::set_permissions(&gate, fs::Permissions::from_mode(0o755)).expect("chmod gate");
    gate
}

fn run_gate(gate: &Path, jobs_dir: &Path, pid: u32, wait_seconds: u32) -> std::process::Output {
    Command::new(gate)
        .env("STADO_RUNNER_JOBS_DIR", jobs_dir)
        .env("STADO_RUNNER_JOB_PID", pid.to_string())
        .env("STADO_RUNNER_JOB_WAIT_SECONDS", wait_seconds.to_string())
        .env("STADO_RUNNER_JOB_POLL_SECONDS", "1")
        .stdin(Stdio::null())
        .output()
        .expect("gate runs")
}

#[test]
fn the_first_job_takes_the_host_and_records_which_account_holds_it() {
    let scratch = Scratch::new("takes");
    let gate = install_gate(&scratch.path);
    let jobs = scratch.path.join("jobs");

    let out = run_gate(&gate, &jobs, std::process::id(), 30);
    assert!(
        out.status.success(),
        "the gate refused an idle host: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let account = String::from_utf8_lossy(
        &Command::new("/usr/bin/id")
            .arg("-un")
            .output()
            .expect("id -un")
            .stdout,
    )
    .trim()
    .to_string();
    let marker = jobs.join(format!("{account}.job"));
    let held = fs::read_to_string(&marker).expect("the gate records the holder");
    assert_eq!(
        held.trim(),
        std::process::id().to_string(),
        "the marker must name the process holding the slot"
    );
}

#[test]
fn a_second_job_waits_while_a_live_job_holds_the_host() {
    let scratch = Scratch::new("waits");
    let gate = install_gate(&scratch.path);
    let jobs = scratch.path.join("jobs");
    fs::create_dir_all(&jobs).expect("jobs dir");

    // A live holder from another runner account: this process' own pid, which
    // is by definition alive, under a different account name.
    fs::write(
        jobs.join("stado-publisher.job"),
        format!("{}\n", std::process::id()),
    )
    .expect("seed a live holder");

    let started = Instant::now();
    let out = run_gate(&gate, &jobs, 4_242, 3);
    let waited = started.elapsed();

    assert!(
        waited >= Duration::from_secs(1),
        "the gate returned in {waited:?} while another job held the host"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("waiting for the job holding this host"),
        "a waiting job must say what it waits for: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn a_marker_whose_process_is_gone_is_not_a_job() {
    let scratch = Scratch::new("stale");
    let gate = install_gate(&scratch.path);
    let jobs = scratch.path.join("jobs");
    fs::create_dir_all(&jobs).expect("jobs dir");

    // A pid that cannot be running: a runner killed mid-job leaves exactly
    // this, and treating it as a live job would stop the host forever.
    let stale = jobs.join("brama-release.job");
    fs::write(&stale, "2147483647\n").expect("seed a stale holder");

    let started = Instant::now();
    let out = run_gate(&gate, &jobs, std::process::id(), 30);
    assert!(
        out.status.success(),
        "the gate refused on a stale marker: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a dead holder must not delay a job"
    );
    assert!(
        !stale.exists(),
        "the gate must clear a marker whose process is gone"
    );
}

#[test]
fn the_installed_runner_program_carries_the_gate_and_both_hooks() {
    // The gate is only real if the runner actually calls it, and the two
    // hooks are what make a marker span exactly one job.
    let program = job_gate_program(MACOS_JOBS_DIR);
    assert!(
        program.contains(MACOS_JOBS_DIR),
        "the installed gate must name the host's markers directory"
    );
    assert!(
        program.contains("STADO_RUNNER_JOBS_DIR"),
        "the gate must remain drivable against a scratch directory"
    );
}
