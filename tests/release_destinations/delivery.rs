use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};
use serde_json::{json, Value};
use stado::deploy::shlex_quote;
use stado::release_pipeline::{DeliveryRunState, ReleaseRun, ReleaseRunState};
use stado::remote::object_store::ObjectRef;
use super::runner::{require, Journey};

pub fn validate_inputs(journey: &Journey) -> Result<(), String> {
    let mut binaries = BTreeSet::new();
    for delivery in journey.manifest["deliveries"].as_array().ok_or("source deliveries are missing")? {
        let argv = delivery["argv"].as_array().ok_or("delivery argv is missing")?;
        let binary = argv.windows(2).find(|pair| pair[0].as_str() == Some("--binary"))
            .and_then(|pair| pair[1].as_str()).ok_or("qualification requires real binary installation deliveries")?;
        binaries.insert(binary.to_owned());
    }
    let versioned = journey.configuration.checks.iter().filter(|check| check.expect_version)
        .map(|check| check.binary.clone()).collect::<BTreeSet<_>>();
    require(!binaries.is_empty() && versioned == binaries, "every installed binary needs a real version observation")?;
    for check in &journey.configuration.checks {
        let mut components = Path::new(&check.binary).components();
        require(matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
                && binaries.contains(&check.binary) && !check.arguments.is_empty(), "consumer checks must invoke installed source binaries")?;
    }
    require(journey.configuration.checks.iter().any(|check| !check.expect_version), "include an actual non-version consumer check, such as the companion sandbox probe")?;
    let targets = journey.configuration.targets.iter().cloned().collect::<BTreeSet<_>>();
    require(targets == journey.configuration.consumer_instances.keys().cloned().collect()
            && journey.configuration.consumer_instances.values().all(|instance| !instance.is_empty())
            && journey.configuration.consumer_instances.values().collect::<BTreeSet<_>>().len() == targets.len(),
            "every dedicated target needs a distinct expected real worker instance")
}

fn expected_deliveries(journey: &Journey) -> Result<BTreeMap<String, String>, String> {
    let registry = journey.registry["targets"].as_array().ok_or("registry targets are missing")?;
    let mut expected = BTreeMap::new();
    for delivery in journey.manifest["deliveries"].as_array().ok_or("source deliveries are missing")? {
        let name = delivery["name"].as_str().ok_or("delivery name is missing")?;
        for target in &journey.configuration.targets {
            let declaration = registry.iter().find(|record| record["name"] == *target).ok_or("target is not registered")?;
            if declaration["release_platform"] == delivery["runner_platform"] {
                expected.insert(format!("{name}--{target}"), target.clone());
            }
        }
    }
    require(expected.values().collect::<BTreeSet<_>>() == journey.configuration.targets.iter().collect(),
            "all dedicated targets must have a matching real installation")?;
    Ok(expected)
}

fn observe_job(journey: &mut Journey, job_id: &str, target: &str) -> Result<String, String> {
    let observation = journey.cli(&["machine", "status", job_id, "--until", "terminal"], true)?;
    let job = &observation["result"]["job"];
    require(job["terminal"].as_bool() == Some(true) && job["completed_at"].is_string()
            && job["error"].is_null() && job["failed_at"].is_null(), &format!("worker did not complete successfully: {observation}"))?;
    let state = job["state"].as_str().ok_or("terminal job has no storage state")?;
    let stored = journey.cli(&["storage", "cat", &format!("{state}/{job_id}.json")], true)?;
    let expected = journey.configuration.consumer_instances.get(target).ok_or("expected worker identity is missing")?;
    require(stored["instance_ref"].as_str() == Some(expected.as_str()), "job was claimed by a different consumer than the selected registry host")?;
    let mut cursor = 0_u64;
    let mut log = String::new();
    loop {
        let page = journey.cli(&["machine", "logs", job_id, "--cursor", &cursor.to_string()], true)?;
        let page = &page["result"];
        log.push_str(page["text"].as_str().ok_or("worker log page has no text")?);
        if page["eof"].as_bool() == Some(true) {
            break;
        }
        let next = page["next_cursor"].as_u64().ok_or("worker log page has no continuation cursor")?;
        require(next > cursor, "completed worker log did not advance its byte cursor")?;
        cursor = next;
    }
    Ok(log)
}

fn consumer_checks(journey: &mut Journey) -> Result<(), String> {
    let targets = journey.configuration.targets.clone();
    let checks = journey.configuration.checks.clone();
    for target in targets {
        for check in &checks {
            let mut command = format!("\"$HOME/.stado/bin/\"{}", shlex_quote(&check.binary));
            for argument in &check.arguments {
                command.push(' ');
                command.push_str(&shlex_quote(argument));
            }
            let request = json!({
                "client_request_id": uuid::Uuid::new_v4().to_string(), "command": command,
                "pinned_host": target, "repo_extras": "",
            });
            let path = journey.output.join(format!("consumer-{}.json", request["client_request_id"].as_str().unwrap()));
            fs::write(&path, serde_json::to_vec_pretty(&request).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
            let submitted = journey.cli(&["machine", "submit", "--request-file", path.to_str().ok_or("non-UTF8 request path")?], true)?;
            let job_id = submitted["result"]["job"]["job_id"].as_str().ok_or("consumer request has no real job identity")?;
            let log = observe_job(journey, job_id, &target)?;
            if check.expect_version {
                require(log.split_whitespace().any(|field| field == journey.configuration.version),
                        "installed consumer did not report the released version")?;
            }
        }
    }
    Ok(())
}

pub fn qualify(journey: &mut Journey) -> Result<(), String> {
    let product = journey.configuration.product.clone();
    let source = journey.configuration.source.to_str().ok_or("non-UTF8 source")?.to_owned();
    let commit = journey.configuration.commit.clone();
    let version = journey.configuration.version.clone();
    let submitted = journey.cli(&["release", "submit", "--source", &source, "--commit", &commit,
        "--version", &version, "--channel", "candidate", "--json"], true)?;
    let submitted: ReleaseRun = serde_json::from_value(submitted).map_err(|error| error.to_string())?;
    let source_object = ObjectRef::parse(&submitted.source_uri).map_err(|error| error.to_string())?;
    let placement_uri = ObjectRef::new(source_object.namespace(), &format!("runs/release-pipeline/{product}/{}/deliveries/placement.json", submitted.run_id))
        .map_err(|error| error.to_string())?.to_string();
    let placement = journey.cli(&["storage", "cat", &placement_uri], true)?;
    require(placement["run_id"] == submitted.run_id && placement["source_sha256"] == submitted.source_sha256
            && placement["manifest_sha256"] == submitted.manifest_sha256, "retained placement is not bound to this exact release")?;
    let expected = expected_deliveries(journey)?;
    let first = journey.configuration.targets[0].clone();
    journey.declare(&[first], true)?;
    let finished = journey.cli(&["release", "resume", &submitted.run_id, "--json"], true)?;
    let finished: ReleaseRun = serde_json::from_value(finished).map_err(|error| error.to_string())?;
    require(finished.state == ReleaseRunState::Completed && finished.source_commit == commit,
            "the exact candidate release did not complete")?;
    require(finished.deliveries.keys().collect::<BTreeSet<_>>() == expected.keys().collect(),
            "actual release deliveries differ from the full frozen destination expansion")?;
    for (name, target) in &expected {
        let delivery = &finished.deliveries[name];
        require(delivery.state == DeliveryRunState::Passed && delivery.receipt_sha256.is_some(),
                &format!("delivery did not pass with its real receipt: {name}"))?;
        observe_job(journey, &delivery.job_id, target)?;
    }
    require(journey.cli(&["storage", "cat", &placement_uri], true)? == placement,
            "a registry edit or resume rewrote frozen placement")?;
    journey.cli(&["release", "destinations", "remove", &product, "--json"], true)?;
    let removed_target = journey.configuration.targets.last().ok_or("no removed target")?.clone();
    let name = expected.iter().find(|(_, target)| **target == removed_target).map(|(name, _)| name).ok_or("removed target has no delivery")?;
    let retry_token = uuid::Uuid::new_v4().to_string();
    let redelivered = journey.cli(&["release", "redeliver", &product, &submitted.run_id, name,
        "--retry-token", &retry_token, "--json"], true)?;
    let job_id = redelivered["job_id"].as_str().ok_or("redelivery has no real job")?;
    observe_job(journey, job_id, &removed_target)?;
    let replay = journey.cli(&["release", "redeliver", &product, &submitted.run_id, name,
        "--retry-token", &retry_token, "--json"], true)?;
    require(replay == redelivered, "retry token submitted a different redelivery after completion")?;
    require(journey.cli(&["storage", "cat", &placement_uri], true)? == placement,
            "redelivery replaced the frozen destinations")?;
    consumer_checks(journey)?;
    journey.cli(&["release", "destinations", "show", &product, "--json"], false)?;
    journey.report["release_run"] = serde_json::to_value(finished).map_err(|error| error.to_string())?;
    journey.report["placement"] = placement;
    journey.report["redelivery"] = redelivered;
    journey.save()
}
