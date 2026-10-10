//! Corrupt only this journey's actual worker output, through the real object CLI.
use super::runner::{require, Journey};
use serde_json::{json, Value};
use stado::release_pipeline::{DeliveryRunState, ReleaseRun, ReleaseRunState, StepStatus};
use stado::remote::object_store::ObjectRef;
use std::fs;
use std::path::PathBuf;

type Case = (String, Option<Vec<u8>>, Vec<String>);

fn cases(original: &[u8]) -> Result<Vec<Case>, String> {
    let receipt: Value = serde_json::from_slice(original).map_err(|error| error.to_string())?;
    let mut cases = vec![
        ("absent".into(), None, vec!["left no delivery-receipt.json".into()]),
        ("malformed".into(), Some(b"{".to_vec()), vec!["does not decode".into(), "EOF".into()]),
    ];
    for (field, label) in [
        ("run_id", "run"), ("job_id", "job"), ("name", "delivery"),
        ("product", "product"), ("version", "version"), ("platform", "platform"),
        ("archive_uri", "archive uri"), ("archive_sha256", "archive sha256"),
        ("manifest_uri", "manifest uri"), ("manifest_sha256", "manifest sha256"),
    ] {
        let mut changed = receipt.clone();
        let written = uuid::Uuid::new_v4().to_string();
        let expected = receipt[field].as_str().ok_or_else(|| format!("real receipt has no {field}"))?;
        require(expected != written, "corruption must differ from the worker's real output")?;
        changed[field] = json!(written);
        cases.push((field.into(), Some(serde_json::to_vec(&changed).map_err(|error| error.to_string())?),
            vec![format!("its {label} is {written}, the request declares {expected}")]));
    }
    let required = receipt["required"].as_bool().ok_or("real receipt has no required flag")?;
    let mut secrets = receipt["secret_env"].as_object().ok_or("real receipt has no secret references")?.clone();
    secrets.insert(uuid::Uuid::new_v4().to_string(), json!(uuid::Uuid::new_v4().to_string()));
    let mut arguments = receipt["argv"].as_array().ok_or("real receipt has no arguments")?.clone();
    arguments.push(json!(uuid::Uuid::new_v4().to_string()));
    require(receipt["schema_version"] != json!(u32::MAX), "real receipt already uses the schema type boundary")?;
    for (field, changed, cause) in [
        ("schema_version", json!(u32::MAX), "its schema version"),
        ("required", json!(!required), "it says required"),
        ("secret_env", json!(secrets), "its secret references"),
        ("argv", json!(arguments), "it ran"),
        ("status", serde_json::to_value(StepStatus::Failed).map_err(|error| error.to_string())?, "its step"),
        ("exit_code", json!(nix::libc::EXIT_FAILURE), "its exit code"),
        ("completed_at", json!(""), "is not RFC 3339"),
    ] {
        let mut value = receipt.clone();
        value[field] = changed;
        cases.push((field.into(), Some(serde_json::to_vec(&value).map_err(|error| error.to_string())?), vec![cause.into()]));
    }
    let mut changed_bytes = original.to_vec();
    changed_bytes.push(b'\n');
    require(serde_json::from_slice::<Value>(&changed_bytes).map_err(|error| error.to_string())? == receipt,
        "digest-only case must preserve the actual receipt's meaning")?;
    cases.push(("accepted-digest".into(), Some(changed_bytes), vec!["hashes to".into(), "accepted".into()]));
    Ok(cases)
}

fn download(journey: &mut Journey, uri: &str, name: &str) -> Result<Vec<u8>, String> {
    let path = journey.output.join(name);
    let binary = PathBuf::from(journey.command().get_program());
    journey.execute(&binary, &["storage", "get", uri, path.to_str().ok_or("non-UTF8 output")?], true, true)?;
    fs::read(path).map_err(|error| error.to_string())
}

fn upload(journey: &mut Journey, uri: &str, name: &str, bytes: &[u8]) -> Result<(), String> {
    let path = journey.output.join(name);
    fs::write(&path, bytes).map_err(|error| error.to_string())?;
    journey.cli(&["storage", "put", uri, path.to_str().ok_or("non-UTF8 output")?, "--json"], true)?;
    require(download(journey, uri, "uploaded-receipt.json")? == bytes, "object upload did not retain the exact case bytes")
}

pub fn qualify(
    journey: &mut Journey,
    run: &ReleaseRun,
    delivery: &str,
    retry_token: &str,
    passed: &Value,
) -> Result<(), String> {
    let job = passed["job_id"].as_str().ok_or("real redelivery has no job identity")?;
    let source = ObjectRef::parse(&run.source_uri).map_err(|error| error.to_string())?;
    let receipt_uri = ObjectRef::new(source.namespace(), &format!("status/{job}/output/delivery-receipt.json"))
        .map_err(|error| error.to_string())?.to_string();
    let transaction_uri = ObjectRef::new(source.namespace(), &format!("runs/release-pipeline/{}/{}/redelivery.json", run.product, run.run_id))
        .map_err(|error| error.to_string())?.to_string();
    let original = download(journey, &receipt_uri, "original-redelivery-receipt.json")?;
    let decoded: Value = serde_json::from_slice(&original).map_err(|error| error.to_string())?;
    require(decoded["job_id"] == job && decoded["run_id"] == run.run_id && decoded["name"] == delivery,
        "only this journey's genuinely completed redelivery output may be altered")?;
    require(decoded["required"].as_bool() == Some(true), "receipt qualification requires a required real delivery")?;
    let accepted = passed["receipt_sha256"].as_str().ok_or("real redelivery has no accepted digest")?;
    let run_uri = ObjectRef::new(source.namespace(), &format!("runs/release-pipeline/{}/run.json", run.run_id))
        .map_err(|error| error.to_string())?.to_string();
    let transaction = download(journey, &transaction_uri, "original-redelivery-transaction.json")?;
    journey.report["receipt_cases"] = json!([]);
    journey.save()?;
    for (name, altered, causes) in cases(&original)? {
        let result = (|| -> Result<(), String> {
            if let Some(bytes) = &altered {
                upload(journey, &receipt_uri, &format!("receipt-{name}.json"), bytes)?;
            } else {
                journey.cli(&["storage", "rm", &receipt_uri, "--json"], true)?;
            }
            for token in [retry_token.to_owned(), uuid::Uuid::new_v4().to_string()] {
                let refused = journey.cli(&[
                    "release", "redeliver", &run.product, &run.run_id, delivery,
                    "--retry-token", &token, "--json",
                ], false)?;
                let error = refused["error"].as_str().ok_or("CLI refusal lost its cause")?;
                require(error.contains(job), &format!("refusal lost actual job {job}: {error}"))?;
                for cause in &causes {
                    require(error.contains(cause), &format!("receipt {name} refusal lost {cause}: {error}"))?;
                }
                require(download(journey, &transaction_uri, "observed-redelivery-transaction.json")? == transaction,
                    "refusal changed the accepted transaction or admitted a new job")?;
            }
            for phase in ["retained-passed", "repeated-failure"] {
                let refused = journey.cli(&["release", "resume", &run.run_id, "--json"], false)?;
                let error = refused["error"].as_str().ok_or("resume refusal lost its cause")?;
                require(error.contains(job), &format!("{phase} lost actual job {job}: {error}"))?;
                for cause in &causes {
                    require(error.contains(cause), &format!("{phase} lost {cause}: {error}"))?;
                }
                let persisted = journey.cli(&["storage", "cat", &run_uri], true)?;
                let persisted: ReleaseRun = serde_json::from_value(persisted).map_err(|error| error.to_string())?;
                let leg = persisted.deliveries.get(delivery).ok_or("failed delivery disappeared")?;
                require(persisted.state == ReleaseRunState::Failed && leg.state == DeliveryRunState::Failed,
                    "receipt refusal was not persisted on the required delivery and run")?;
                require(leg.job_id == job && leg.receipt_sha256.as_deref() == Some(accepted),
                    "a failed pass replaced the genuine job identity or accepted proof digest")?;
                let retained = leg.failure.as_deref().ok_or("failed receipt has no retained cause")?;
                for cause in &causes {
                    require(retained.contains(cause), &format!("{phase} persisted cause omitted {cause}: {retained}"))?;
                }
            }
            Ok(())
        })();
        // Always restore this journey's genuine bytes, including after a failed assertion.
        let restored = (|| -> Result<(), String> {
            upload(journey, &receipt_uri, "restore-redelivery-receipt.json", &original)?;
            let completed = journey.cli(&["release", "resume", &run.run_id, "--json"], true)?;
            let completed: ReleaseRun = serde_json::from_value(completed).map_err(|error| error.to_string())?;
            let leg = completed.deliveries.get(delivery).ok_or("restored delivery disappeared")?;
            require(completed.state == ReleaseRunState::Completed && leg.state == DeliveryRunState::Passed
                    && leg.job_id == job && leg.receipt_sha256.as_deref() == Some(accepted) && leg.failure.is_none(),
                "restoring genuine receipt bytes did not recover the same accepted delivery")?;
            let persisted = journey.cli(&["storage", "cat", &run_uri], true)?;
            require(serde_json::to_value(completed).map_err(|error| error.to_string())? == persisted,
                "restored resume response differs from the persisted run")
        })();
        journey.report["receipt_cases"].as_array_mut().ok_or("receipt report is not an array")?.push(json!({
            "case": name, "failure": result.as_ref().err(), "restoration_failure": restored.as_ref().err(),
            "receipt_uri": receipt_uri, "job_id": job,
        }));
        journey.save()?;
        if let Err(cause) = restored {
            return Err(format!("receipt case {name} result {result:?}; restoring {receipt_uri} failed: {cause}"));
        }
        result?;
        let replay = journey.cli(&[
            "release", "redeliver", &run.product, &run.run_id, delivery,
            "--retry-token", retry_token, "--json",
        ], true)?;
        require(replay == *passed, "restored genuine proof did not replay the same accepted job")?;
    }
    require(download(journey, &receipt_uri, "final-redelivery-receipt.json")? == original,
        "qualification did not restore the actual worker receipt")?;
    require(download(journey, &transaction_uri, "final-redelivery-transaction.json")? == transaction,
        "qualification changed the accepted transaction")?;
    journey.report["receipt_integrity"] = json!({"passed": true, "receipt_restored": true, "transaction_unchanged": true});
    journey.save()
}
