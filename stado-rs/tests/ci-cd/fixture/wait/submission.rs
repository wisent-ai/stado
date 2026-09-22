//! Waiting for a submission to reach the state the assertions are about, and
//! keeping everything the run produced beside it.
//!
//! Nothing here sleeps for a guess: each wait names what it is waiting for and
//! fails with the store as it stood, because a journey that times out with no
//! record of what the fleet held is a failure nobody can read afterwards.

use super::super::*;

pub(crate) fn wait_for_recovery_delivery(
    child: &mut Child,
    agent: &mut Child,
    home: &Path,
    storage: &Path,
    consumer: &str,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Ok(entries) = fs::read_dir(storage.join("queue")) {
            for entry in entries.flatten() {
                let Ok(bytes) = fs::read(entry.path()) else {
                    continue;
                };
                let Ok(job) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                if job["command"]
                    == stado::primitives::constants::PRODUCT_RELEASE_DELIVERY_JOB_COMMAND
                    && job["pinned_host"] == consumer
                {
                    return job;
                }
            }
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!(
                "release submit exited before queuing the recovery delivery: {status}\n\
             submit stdout:\n{}\nsubmit stderr:\n{}\nstore:{}",
                fs::read_to_string(home.join("submit.out")).unwrap_or_default(),
                fs::read_to_string(home.join("submit.err")).unwrap_or_default(),
                store_snapshot(storage)
            );
        }
        if let Some(status) = agent.try_wait().unwrap() {
            let _ = child.kill();
            panic!(
                "builder exited before the recovery delivery was queued: {status}\n\
             agent stdout:\n{}\nagent stderr:\n{}",
                fs::read_to_string(home.join("agent.out")).unwrap_or_default(),
                fs::read_to_string(home.join("agent.err")).unwrap_or_default()
            );
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
            "release submit queued no recovery delivery within 180 seconds\n\
             submit stdout:\n{}\nsubmit stderr:\n{}\nagent stdout:\n{}\nagent stderr:\n{}\nstore:{}",
            fs::read_to_string(home.join("submit.out")).unwrap_or_default(),
            fs::read_to_string(home.join("submit.err")).unwrap_or_default(),
            fs::read_to_string(home.join("agent.out")).unwrap_or_default(),
            fs::read_to_string(home.join("agent.err")).unwrap_or_default(),
            store_snapshot(storage)
        );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub(crate) fn wait_for_queued_release_build(
    child: &mut Child,
    home: &Path,
    storage: &Path,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(entries) = fs::read_dir(storage.join("queue")) {
            for entry in entries.flatten() {
                let Ok(bytes) = fs::read(entry.path()) else {
                    continue;
                };
                let Ok(job) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                if job["state"] == "queued" && job["job_id"].is_string() {
                    return job;
                }
            }
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!(
                "release submit exited before queuing its build: {status}\n\
             submit stdout:\n{}\nsubmit stderr:\n{}\nstore:{}",
                fs::read_to_string(home.join("submit-first.out")).unwrap_or_default(),
                fs::read_to_string(home.join("submit-first.err")).unwrap_or_default(),
                store_snapshot(storage)
            );
        }
        assert!(
            Instant::now() < deadline,
            "release submit queued no build within 30 seconds\nstore:{}",
            store_snapshot(storage)
        );
        thread::sleep(Duration::from_millis(100));
    }
}

pub(crate) fn store_snapshot(storage: &Path) -> String {
    let mut out = String::new();
    for prefix in ["queue", "running", "failed", "completed", "capacity"] {
        let path = storage.join(prefix);
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            out.push_str(&format!(
                "\n== {prefix}/{} ==\n{}",
                entry.file_name().to_string_lossy(),
                fs::read_to_string(entry.path()).unwrap_or_else(|_| "<binary>".into())
            ));
        }
    }
    out
}

/// Wait for a release submission to reach its end.
///
/// `stado release submit` ends when the builds are queued; in the fleet the
/// control host's release agent finishes the run once they are done. These
/// journeys have no such agent, so when the submission returns `waiting`
/// the same journey continues it with `stado release resume`, under the same
/// watch over the builder and the same deadline, and `submit.out` ends up
/// holding what the finished run reported — exactly what the assertions read.
pub(crate) fn wait_for_submit(
    child: &mut Child,
    agent: &mut Child,
    home: &Path,
    storage: &Path,
    vault: &SkarbiecFixture,
) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(180);
    let status = wait_for_release_process(child, agent, home, storage, deadline);
    if !status.success() {
        return status;
    }
    let Some(mut resume) = resume_after_queueing(home, storage, vault, "submit") else {
        return status;
    };
    wait_for_release_process(&mut resume, agent, home, storage, deadline)
}

/// The process a watcher follows after `submit` queued its builds: `submit`
/// itself while it runs, then `stado release resume` on the run it recorded.
/// A journey that watches the store while "the submission" runs - for its
/// run state, a queued delivery, a builder's claim - watches this child.
/// `name` is the stem of the retained output files, `submit` by default.
pub(crate) fn follow_submission(
    mut child: Child,
    home: &Path,
    storage: &Path,
    vault: &SkarbiecFixture,
    name: &str,
) -> Child {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            if !status.success() {
                return child;
            }
            break;
        }
        assert!(
            Instant::now() < deadline,
            "release submit did not queue its builds within 180 seconds\nsubmit stdout:\n{}\nsubmit stderr:\n{}",
            fs::read_to_string(home.join(format!("{name}.out"))).unwrap_or_default(),
            fs::read_to_string(home.join(format!("{name}.err"))).unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(100));
    }
    resume_after_queueing(home, storage, vault, name).unwrap_or(child)
}

/// When `<name>.out` holds a run in state `waiting`, the resume of that run,
/// writing over `<name>.out` and appending to `<name>.err` so the retained
/// files end with what the finished run reported.
fn resume_after_queueing(
    home: &Path,
    storage: &Path,
    vault: &SkarbiecFixture,
    name: &str,
) -> Option<Child> {
    let queued: Option<Value> = fs::read(home.join(format!("{name}.out")))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let run_id = queued
        .filter(|run| run["state"] == "waiting")
        .and_then(|run| run["run_id"].as_str().map(str::to_owned))?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home, storage, vault);
    Some(
        command
            .args(["release", "resume", &run_id, "--json"])
            .stdout(Stdio::from(
                File::create(home.join(format!("{name}.out"))).unwrap(),
            ))
            .stderr(Stdio::from(
                fs::OpenOptions::new()
                    .append(true)
                    .open(home.join(format!("{name}.err")))
                    .unwrap(),
            ))
            .spawn()
            .unwrap(),
    )
}

fn wait_for_release_process(
    child: &mut Child,
    agent: &mut Child,
    home: &Path,
    storage: &Path,
    deadline: Instant,
) -> std::process::ExitStatus {
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if let Some(status) = agent.try_wait().unwrap() {
            let _ = child.kill();
            panic!(
            "agent exited while release submit waited: {status}\nagent stdout:\n{}\nagent stderr:\n{}",
            fs::read_to_string(home.join("agent.out")).unwrap_or_default(),
            fs::read_to_string(home.join("agent.err")).unwrap_or_default()
        );
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
            "release submit did not finish within 180 seconds\nsubmit stdout:\n{}\nsubmit stderr:\n{}\nagent stdout:\n{}\nagent stderr:\n{}\nstore:{}",
            fs::read_to_string(home.join("submit.out")).unwrap_or_default(),
            fs::read_to_string(home.join("submit.err")).unwrap_or_default(),
            fs::read_to_string(home.join("agent.out")).unwrap_or_default(),
            fs::read_to_string(home.join("agent.err")).unwrap_or_default(),
            store_snapshot(storage)
        );
        }
        thread::sleep(Duration::from_millis(100));
    }
}
