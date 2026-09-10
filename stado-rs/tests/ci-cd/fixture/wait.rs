use super::*;
pub(crate) struct Running(pub(crate) Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub(crate) fn wait_for_claimable_capacity(storage: &Path, home: &Path, agent: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(300);
    let since = chrono::Utc::now();
    loop {
        if let Ok(entries) = fs::read_dir(storage.join("capacity")) {
            for entry in entries.flatten() {
                // Only the published object is readiness, not its atomic-write candidate.
                if entry.file_name().to_string_lossy().starts_with('.')
                    || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
                {
                    continue;
                }
                let Ok(bytes) = fs::read(entry.path()) else {
                    continue;
                };
                if serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .is_some_and(|capacity| {
                        capacity["accepting_jobs"] == true
                            && capacity["published_at"]
                                .as_str()
                                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                                .is_some_and(|published| published >= since)
                    })
                {
                    return;
                }
            }
        }
        if let Some(status) = agent.try_wait().unwrap() {
            panic!(
                "agent exited before accepting work: {status}\nstdout:\n{}\nstderr:\n{}",
                fs::read_to_string(home.join("agent.out")).unwrap_or_default(),
                fs::read_to_string(home.join("agent.err")).unwrap_or_default()
            );
        }
        if Instant::now() >= deadline {
            let _ = agent.kill();
            let _ = agent.wait();
            panic!(
                "agent accepted no work within 300 seconds\nstore:{}",
                store_snapshot(storage)
            );
        }
        thread::sleep(Duration::from_millis(250));
    }
}

pub(crate) fn seed_stale_capacity(storage: &Path, consumer: &str) {
    let capacity = storage.join("capacity");
    fs::create_dir_all(&capacity).unwrap();
    let path = capacity.join(format!("{consumer}.json"));
    fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "consumer_id": consumer,
            "kind": "local",
            "published_at": "2026-01-01T00:00:00Z",
            "free_slots": {},
            "diag": {
                "disk_pressure_active": true,
                "disk_pressure_unresolved": true
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let stale = SystemTime::now() - Duration::from_secs(240);
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(stale))
        .unwrap();
}

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
                if job["command"] == stado::primitives::constants::PRODUCT_RELEASE_DELIVERY_JOB_COMMAND
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

pub(crate) fn wait_for_submit(
    child: &mut Child,
    agent: &mut Child,
    home: &Path,
    storage: &Path,
) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(180);
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
