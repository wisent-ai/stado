//! A host's own routine fires and runs from its host store while the fleet
//! store does not answer.
//!
//! One isolated deployment: `schedule create` declares a routine into the
//! host store (`~/.stado/host-store`, a local store at that path), then the
//! real `stado serve --worker --standalone` runs with its fleet store pointed
//! at a loopback port allocated by the product but holding no listener.
//! The real connection refusal must not prevent the worker's host pass firing
//! routine, run its command as a child shell, append its output to the
//! routine's log, and record the job completed in the host store.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::deployment::Deployment;
use serde_json::{json, Value};

/// Every job document under `prefix/` of the store at `root`.
fn jobs(root: &Path, prefix: &str) -> Vec<Value> {
    let directory = root.join(prefix);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => panic!("{} cannot be listed: {error}", directory.display()),
    };
    entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| serde_json::from_slice(&fs::read(&path).unwrap()).unwrap())
        .collect()
}

#[test]
fn a_host_routine_fires_and_runs_while_the_fleet_store_does_not_answer() {
    let mut deployment = Deployment::start("host-routine");
    let host_store = deployment.root.join(".stado").join("host-store");
    let host_path = host_store
        .to_str()
        .expect("the run directory is UTF-8")
        .to_string();

    // Declared through the real schedule command into the host store.
    deployment.set("WC_STORAGE_BACKEND", "local");
    deployment.set("WC_LOCAL_STORAGE_PATH", &host_path);
    deployment.cli(&[
        "schedule",
        "create",
        "--cron",
        "* * * * *",
        "echo host-routine-ran",
    ]);
    let listed: Value =
        serde_json::from_str(&deployment.cli(&["schedule", "list", "--json"])).unwrap();
    deployment.report["host_schedules"] = listed.clone();
    let Some(schedule) = listed
        .as_array()
        .and_then(|schedules| schedules.first())
        .and_then(|declared| declared["schedule_id"].as_str())
        .map(str::to_string)
    else {
        panic!("the host store lists the declared routine: {listed}");
    };

    let port = deployment
        .cli(&["host", "local", "port"])
        .trim()
        .parse::<u16>()
        .expect("the product returns an allocated local port");
    let unanswered = format!("http://{}:{port}", std::net::Ipv4Addr::LOCALHOST);
    let token = deployment.root.join("object-api.token");
    fs::write(&token, uuid::Uuid::new_v4().to_string()).unwrap();
    fs::set_permissions(
        &token,
        fs::Permissions::from_mode(stado::primitives::file_mode::owner_read_write()),
    )
    .unwrap();
    deployment.set("WC_STORAGE_BACKEND", "stado");
    deployment.set("WC_STADO_STORAGE_URL", &unanswered);
    deployment.set("WC_STADO_STORAGE_NAMESPACE", "host-routine");
    deployment.set("WC_STADO_STORAGE_TOKEN_FILE", token.to_str().unwrap());
    deployment.report["fleet_store"] = json!(unanswered);

    let lines = deployment.serve(&[
        "--worker",
        "--standalone",
        "--kind",
        "local",
        "--poll-seconds",
        "1",
    ]);
    let mut done = Vec::new();
    for line in lines.iter() {
        if line.contains("the fleet store did not answer") && line.contains(&unanswered) {
            deployment.report["fleet_refusal"] = json!(line);
        }
        if !line.starts_with("[host-routines]") {
            continue;
        }
        deployment.report["host_observation"] = json!(line);
        let failed = jobs(&host_store, "failed");
        assert!(
            !failed
                .iter()
                .any(|job| job["schedule_id"] == json!(schedule)),
            "the host routine failed: {failed:?}"
        );
        let observed = jobs(&host_store, "completed");
        if observed
            .iter()
            .any(|job| job["schedule_id"] == json!(schedule))
        {
            done = observed;
            break;
        }
    }
    assert!(
        deployment.report["fleet_refusal"].is_string(),
        "the real worker must observe the unavailable fleet store"
    );
    deployment.report["completed_jobs"] = json!(done);
    let Some(job) = done
        .iter()
        .find(|job| job["schedule_id"] == json!(schedule))
    else {
        panic!("the routine's job completed in the host store: {done:?}");
    };
    let id = job["job_id"].as_str().expect("a job id").to_string();
    let log = fs::read_to_string(host_store.join("logs").join(format!("{id}.log")))
        .expect("the routine's log is kept in the host store");
    deployment.report["log"] = json!(log);
    assert!(
        log.contains("host-routine-ran"),
        "the routine's command ran: {log}"
    );
    deployment.pass();
}

#[test]
fn selected_store_keeps_replay_and_refusals_local() {
    const CHILD: &str = "STADO_SELECTED_STORE_QUALIFICATION_HOME";
    const CASE: &str = "host_routine::selected_store_keeps_replay_and_refusals_local";
    if let Some(home) = std::env::var_os(CHILD) {
        let home = std::path::PathBuf::from(home);
        let allowed = crate::deployment::deployments_root()
            .canonicalize()
            .unwrap();
        let home = home.canonicalize().unwrap();
        assert_eq!(home.parent(), Some(allowed.as_path()));
        assert_eq!(
            std::env::var_os("HOME"),
            Some(home.clone().into_os_string())
        );
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(qualify_selected_store(&home));
        return;
    }

    let mut deployment = Deployment::start("selected-store-submission");
    let port = deployment.cli(&["host", "local", "port"]);
    let port: u16 = port.trim().parse().unwrap();
    let endpoint = format!("http://{}:{port}", std::net::Ipv4Addr::LOCALHOST);
    let token = deployment.root.join("object-api.token");
    fs::write(&token, uuid::Uuid::new_v4().to_string()).unwrap();
    fs::set_permissions(
        &token,
        fs::Permissions::from_mode(stado::primitives::file_mode::owner_read_write()),
    )
    .unwrap();
    deployment.set("WC_STORAGE_BACKEND", "stado");
    deployment.set("WC_STADO_STORAGE_URL", &endpoint);
    deployment.set("WC_STADO_STORAGE_NAMESPACE", "selected-store-submission");
    deployment.set("WC_STADO_STORAGE_TOKEN_FILE", token.to_str().unwrap());
    let executable = std::env::current_exe().unwrap();
    let arguments = ["--exact", CASE, "--nocapture"];
    let output = deployment
        .native(&executable)
        .args(arguments)
        .env(CHILD, &deployment.root)
        .output()
        .expect("the isolated native API qualification starts");
    deployment.report["qualification"] = json!({
        "executable": executable, "arguments": arguments, "exit_status": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
        "fleet_endpoint": endpoint,
    });
    deployment.save();
    assert!(
        output.status.success(),
        "{}",
        deployment.report["qualification"]
    );
    deployment.report["submission"] =
        serde_json::from_slice(&fs::read(deployment.root.join("submission.json")).unwrap())
            .unwrap();
    deployment.pass();
}

async fn qualify_selected_store(home: &Path) {
    use stado::queue::submit::{SubmitError, SubmitOptions};
    use stado::queue::{JobStorage, LocalBackend};
    let root = home.join(".stado/host-store");
    let bucket = root.to_str().unwrap();
    let store = JobStorage::with_backend_and_bucket(
        std::sync::Arc::new(LocalBackend::new(bucket).unwrap()),
        stado::capabilities::StorageAdapter::Local.id(),
        bucket,
    );
    let commands = ["echo selected-store-command".to_string()];
    let mut options = SubmitOptions {
        bucket: bucket.to_string(),
        run_id: "host-bound-run".to_string(),
        ..SubmitOptions::default()
    };
    let accepted = store.submit_batch(&commands, &options).await.unwrap();
    let manifest_path = format!("{}/{}.json", stado::queue::runs::RUN_PREFIX, options.run_id);
    let manifest = store.download_text(&manifest_path).await.unwrap().unwrap();
    let replay = store.submit_batch(&commands, &options).await.unwrap();
    assert_eq!(
        serde_json::to_value(&replay).unwrap(),
        serde_json::to_value(&accepted).unwrap()
    );
    let queued_before = store.list_jobs("queue").await.unwrap();
    assert_eq!(
        serde_json::to_value(&queued_before).unwrap(),
        serde_json::to_value(&accepted).unwrap()
    );

    let changed = match store
        .submit_batch(&["echo different-request".to_string()], &options)
        .await
    {
        Err(SubmitError::Validation(cause)) => cause,
        other => panic!("a changed immutable request must be refused before writing: {other:?}"),
    };
    assert!(changed.contains(&options.run_id), "{changed}");
    options.bucket = "a-different-selected-store".to_string();
    options.run_id = "refused-bucket-run".to_string();
    let mismatch = match store.submit_batch(&commands, &options).await {
        Err(SubmitError::Validation(cause)) => cause,
        other => panic!("a different bucket must be refused before writing: {other:?}"),
    };
    assert!(
        mismatch.contains(&options.bucket) && mismatch.contains(bucket),
        "{mismatch}"
    );
    assert!(store
        .download_text(&format!(
            "{}/{}.json",
            stado::queue::runs::RUN_PREFIX,
            options.run_id
        ))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .download_text(&manifest_path)
            .await
            .unwrap()
            .as_deref(),
        Some(manifest.as_str())
    );
    let queued_after = store.list_jobs("queue").await.unwrap();
    assert_eq!(
        serde_json::to_value(&queued_after).unwrap(),
        serde_json::to_value(&queued_before).unwrap()
    );
    fs::write(
        home.join("submission.json"),
        serde_json::to_vec_pretty(&json!({
            "selected_store": root, "accepted": accepted, "replay": replay,
            "request_refusal": changed, "bucket_refusal": mismatch,
            "manifest": serde_json::from_str::<Value>(&manifest).unwrap(), "queue": queued_after,
        }))
        .unwrap(),
    )
    .unwrap();
}
