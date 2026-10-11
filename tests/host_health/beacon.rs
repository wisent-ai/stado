//! The host's one Stado process publishes the host's health beacon into the
//! fleet store its queue roles use — the store `stado host beacon list`
//! and `stado service list` read. One isolated deployment: `config init`
//! seeds a local registry naming this machine, `stado serve --api
//! --api-local-store … --health-interval-seconds 1` runs as the real product,
//! and the beacon is read back through `stado host beacon list` and from
//! the object that reader names.
mod deployment;

use deployment::Deployment;
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::sync::mpsc;

#[test]
fn the_host_process_publishes_its_beacon_where_the_fleet_reads_it() {
    let mut deployment = Deployment::start();
    let registry: Value =
        serde_json::from_slice(&fs::read(deployment.store().join("registry.json")).unwrap())
            .unwrap();
    let declared = registry["targets"][0]["name"].as_str().unwrap().to_string();
    assert!(
        !deployment.store().join("host_health").exists(),
        "a fresh deployment must hold no beacon before the process runs"
    );

    let slug = deployment.serve();
    let ages = deployment.cli(&["host", "beacon", "list", "--json"]);
    let ages: Value = serde_json::from_str(&ages).unwrap();
    let row = ages["hosts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["host"] == declared)
        .cloned()
        .unwrap_or_else(|| panic!("beacon-age must list {declared}: {ages}"));
    assert_eq!(row["status"], "reported", "{row}");
    assert!(
        row["age_seconds"].as_i64().is_some_and(|age| age >= 0),
        "beacon list must read the stored beacon: {row}"
    );
    let beacon_path = deployment.store().join(
        row["beacon"]
            .as_str()
            .expect("beacon list names the beacon object"),
    );
    assert_eq!(
        beacon_path.file_name().unwrap().to_string_lossy(),
        format!("{slug}.json"),
        "the stored object is the one the process announced"
    );
    let beacon: Value = serde_json::from_slice(&fs::read(&beacon_path).unwrap()).unwrap();
    assert_eq!(beacon["host"], slug);
    assert!(beacon["reported_at"].is_string(), "{beacon}");
    assert!(beacon["units"].is_object(), "{beacon}");
    assert!(
        beacon.get("link").is_some(),
        "a beacon about this machine carries its link block: {beacon}"
    );
    // The beacon states when its publisher will publish again: at least one
    // health period (one second here) after it was written, so a reader
    // judges it by that promise and not by a window of its own.
    let stamp = |field: &str| {
        chrono::DateTime::parse_from_rfc3339(beacon[field].as_str().unwrap_or_default())
            .unwrap_or_else(|error| panic!("{field} must be RFC 3339 ({error}): {beacon}"))
    };
    assert!(
        stamp("next_by") - stamp("reported_at") >= chrono::Duration::seconds(1),
        "next_by must be at least the health period after reported_at: {beacon}"
    );
    assert!(
        beacon.get("stale_after_seconds").is_none(),
        "the publisher states its own next publication, not a window: {beacon}"
    );
    deployment.report["beacon"] = beacon;
    deployment.report["beacon_age"] = row;
    deployment.pass();
}

/// A Stado release this host recorded as failed reaches the rest of the fleet
/// through its beacon: `stado release failed` names the host by its beacon
/// slug, the version and the cause the host wrote, which is what every other
/// host's delivery refuses it on. The record is the tested release's own.
#[test]
fn a_release_failed_on_this_host_is_listed_for_the_fleet() {
    let mut deployment = Deployment::start();
    let local: Value =
        serde_json::from_str(&deployment.cli(&["release", "local", "list", "--json"])).unwrap();
    let platform = local["platform"]
        .as_str()
        .expect("release local list names this host's platform")
        .to_string();
    let version = env!("CARGO_PKG_VERSION");
    let cause = "this resolver is declared to serve the store it must read first. Retrying cannot resolve it.";
    let directory = deployment
        .home
        .join(".stado/releases/stado")
        .join(version)
        .join(&platform);
    fs::create_dir_all(&directory).unwrap();
    let at = chrono::Utc::now().to_rfc3339();
    fs::write(
        directory.join("failed-on-this-host.json"),
        json!({ "version": version, "at": at, "cause": cause }).to_string(),
    )
    .unwrap();

    let slug = deployment.serve();
    let listed: Value =
        serde_json::from_str(&deployment.cli(&["release", "failed", "--json"])).unwrap();
    let host = listed["hosts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["host"] == slug)
        .cloned()
        .expect("release failed names this host by its beacon slug");
    let failed = host["failed"]
        .as_array()
        .and_then(|failed| failed.first())
        .cloned()
        .expect("this host's beacon publishes the release it recorded as failed");
    assert_eq!(failed["version"], version, "{failed}");
    assert_eq!(failed["platform"], platform.as_str(), "{failed}");
    assert_eq!(failed["record"]["cause"], cause, "{failed}");
    assert_eq!(failed["record"]["at"], at.as_str(), "{failed}");

    let text = deployment.cli(&["release", "failed"]);
    assert!(
        text.contains(&format!("{slug}  Stado {version} failed and was rolled back")),
        "{text}"
    );
    deployment.report["failed_releases"] = listed;
    deployment.pass();
}

/// A fleet store the process cannot open — here a Stado object API on a port
/// nothing listens on, the state another host's vault outage leaves behind
/// (`503 object authorization unavailable`) — skips beacons and leaves the
/// host process running. It used to end the whole process at startup, and
/// with it the resolver every local client reads through.
#[test]
fn the_host_process_stays_up_when_its_fleet_store_cannot_be_opened() {
    let mut deployment = Deployment::start();
    let token = deployment.root.join("store-token");
    fs::write(&token, "test-token").unwrap();
    let token = token.to_string_lossy().into_owned();
    deployment.cli(&["config", "set", "storage.stado.url", "http://127.0.0.1:9"]);
    deployment.cli(&["config", "set", "storage.stado.token_file", &token]);
    deployment.cli(&["config", "set", "storage.backend", "stado"]);

    let args = ["serve", "--health-interval-seconds", "1"];
    let mut child = deployment
        .command()
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    deployment.report["service"] = json!({"arguments": args, "pid": child.id()});
    let (sender, lines) = mpsc::channel();
    let stderr = child.stderr.take().unwrap();
    let stderr_path = deployment.root.join("service.stderr");
    deployment.logs.push(std::thread::spawn(move || {
        let mut file = File::create(stderr_path).unwrap();
        for line in BufReader::new(stderr).lines() {
            let line = line.unwrap();
            writeln!(file, "{line}").unwrap();
            file.flush().unwrap();
            let _ = sender.send(line);
        }
    }));
    deployment.child = Some(child);
    deployment.save();

    // Two skipped beacons prove the role keeps ticking past the refusal.
    let mut skipped = 0;
    for line in lines.iter() {
        if line.contains("the fleet store could not be opened, so this beacon is not published") {
            skipped += 1;
            if skipped == 2 {
                break;
            }
        }
    }
    assert_eq!(
        skipped, 2,
        "the host-health role must report each skipped beacon; inspect service.stderr"
    );
    let running = deployment.child.as_mut().unwrap().try_wait().unwrap();
    assert!(
        running.is_none(),
        "the host process must keep running while its fleet store is unreachable: {running:?}"
    );
    deployment.report["skipped_beacons"] = json!(skipped);
    deployment.pass();
}
