//! Waiting for the fleet to be in a state a journey can start from: a host
//! whose published capacity a builder can actually claim, and the stale
//! publication that proves a journey refuses to claim against an old reading.

use super::super::*;

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
