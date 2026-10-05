//! Real CLI and native-API reads. The live case requires an operator-declared
//! qualification configuration and a running local allocation whose requested
//! provider is non-local. It never seeds price books or invents provider output.
#[path = "../desktop_api/fixture.rs"]
mod fixture;
use fixture::Service;
use serde_json::{json, Value};
use std::path::Path;

fn payload(receipt: Value) -> Value {
    assert_eq!(receipt["ok"], true, "{receipt}");
    assert_eq!(receipt["exit_code"], 0, "{receipt}");
    assert_eq!(receipt["stdout_truncated"], false, "{receipt}");
    assert_eq!(receipt["stderr_truncated"], false, "{receipt}");
    serde_json::from_str(receipt["stdout"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn unknown_allocations_remain_unpriced_through_cli_and_native_api() {
    let mut service = Service::start();
    let id = format!("unallocated-{}", uuid::Uuid::new_v4());
    let lookup = service.execute(
        Path::new(env!("CARGO_BIN_EXE_stado")),
        &["machine", "status", &id],
    );
    let lookup: Value = serde_json::from_slice(&lookup.stdout).unwrap();
    assert_eq!(
        lookup["error"]["code"], "NOT_FOUND",
        "the actual queue lookup must work: {lookup}"
    );
    let before = service.persisted();
    let args = ["cost", "quote", "--json", "--", id.as_str()];
    let cli: Value = serde_json::from_str(&service.cli(&args)).unwrap();
    let api = payload(service.call(json!({"args": args}), 200).await);
    for report in [&cli, &api] {
        assert_eq!(report["complete"], false);
        let rows = report["quotes"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["job_id"], id);
        assert_eq!(
            rows[0]["quote"],
            Value::Null,
            "unknown is not a zero-dollar allocation"
        );
        assert_eq!(rows[0]["allocation"], Value::Null);
        assert!(
            rows[0]["error"].is_string(),
            "the refusal must retain its cause"
        );
    }
    assert_eq!(
        service.persisted(),
        before,
        "read-only quotes changed the declaration"
    );
    service.pass();
}

fn quoted_local<'a>(report: &'a Value, id: &str, expected: &Value) -> &'a Value {
    let row = report["quotes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["job_id"] == id)
        .unwrap();
    assert_eq!(
        row["error"],
        Value::Null,
        "real allocation was not priced: {report}"
    );
    let job = &row["allocation"]["job"];
    assert_eq!(job["job_id"], id);
    assert_eq!(
        job["state"], "running",
        "fixture must retain its running allocation"
    );
    assert!(job["instance_ref"].as_str().unwrap().starts_with("local@"));
    assert_eq!(job["allocation_kind"], "agent");
    let worker = &job["worker_allocation"];
    assert_eq!(worker["error"], Value::Null, "{worker}");
    assert_eq!(worker["resource"]["provider"], "local", "{worker}");
    assert_eq!(
        job["instance_ref"].as_str().unwrap().strip_prefix("local@"),
        worker["host"].as_str(),
        "the rate must belong to this execution's worker"
    );
    assert_ne!(
        job["provider"], "local",
        "qualification requires requested and actual provider to differ"
    );
    let quote = &row["quote"];
    assert_eq!(
        quote["provider"], "local",
        "the request provider was priced instead of the allocation"
    );
    assert_eq!(quote["source"], "autonomy policy");
    assert_eq!(quote["sku"], expected["sku"]);
    assert_eq!(quote["hourly_usd"], expected["hourly_usd"]);
    assert_eq!(quote["currency"], expected["currency"]);
    assert_eq!(quote["unit"], expected["unit"]);
    quote
}

#[tokio::test]
async fn real_local_allocation_uses_its_declared_quote_not_its_requested_provider() {
    let mut service = Service::start_with_configuration("STADO_COST_QUALIFICATION_CONFIG");
    let job = service.input("STADO_COST_QUOTED_JOB_ID");
    let before = service.persisted();
    let book: Value = serde_json::from_str(&service.cli(&["cost", "prices", "--json"])).unwrap();
    let expected = book["quotes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|quote| quote["provider"] == "local" && quote["source"] == "autonomy policy")
        .expect("the real producer must have published the qualification policy's local quote")
        .clone();
    service.observe("provider_price_book", book);
    let api_book = payload(
        service
            .call(json!({"args": ["cost", "prices", "--json"]}), 200)
            .await,
    );
    assert!(api_book["quotes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|quote| quote["provider"] == expected["provider"]
            && quote["sku"] == expected["sku"]
            && quote["hourly_usd"] == expected["hourly_usd"]));
    let args = ["cost", "quote", "--json", "--", job.as_str()];
    let cli: Value = serde_json::from_str(&service.cli(&args)).unwrap();
    quoted_local(&cli, &job, &expected);
    let api = payload(service.call(json!({"args": args}), 200).await);
    quoted_local(&api, &job, &expected);
    service.observe("allocation_quote_cli", cli);
    service.observe("allocation_quote_native_api", api);
    assert_eq!(
        service.persisted(),
        before,
        "cost reads changed the qualification configuration"
    );
    service.pass();
}

#[tokio::test]
async fn cloud_agent_reference_uses_observed_cloud_identity_not_local_policy() {
    use stado::models::WorkerResource;
    let mut service = Service::start_with_configuration("STADO_COST_QUALIFICATION_CONFIG");
    let job = service.input("STADO_COST_QUOTED_CLOUD_JOB_ID");
    let before = service.persisted();
    let book: Value = serde_json::from_str(&service.cli(&["cost", "prices", "--json"])).unwrap();
    let args = ["cost", "quote", "--json", "--", job.as_str()];
    let cli: Value = serde_json::from_str(&service.cli(&args)).unwrap();
    let api = payload(service.call(json!({"args": args}), 200).await);
    for report in [&cli, &api] {
        let row = report["quotes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["job_id"] == job)
            .unwrap();
        assert_eq!(
            row["error"],
            Value::Null,
            "real cloud quote is blocked: {report}"
        );
        let execution = &row["allocation"]["job"];
        assert_eq!(execution["state"], "running");
        assert_eq!(execution["allocation_kind"], "agent");
        let worker = &execution["worker_allocation"];
        assert_eq!(worker["error"], Value::Null, "{worker}");
        assert_eq!(
            execution["instance_ref"]
                .as_str()
                .unwrap()
                .strip_prefix("local@"),
            worker["host"].as_str()
        );
        let identity: WorkerResource = serde_json::from_value(worker["resource"].clone()).unwrap();
        let provider = identity.provider().as_str();
        let inventory = &row["allocation"]["resource"];
        assert_eq!(inventory["provider"], provider);
        match &identity {
            WorkerResource::Aws {
                account_id,
                region,
                instance_id,
            } => {
                assert_eq!(inventory["account"].as_str(), Some(account_id.as_str()));
                assert_eq!(inventory["region"].as_str(), Some(region.as_str()));
                assert_eq!(
                    inventory["native_reference"].as_str(),
                    Some(instance_id.as_str())
                );
                assert_eq!(
                    inventory["evidence"]["account_id"].as_str(),
                    Some(account_id.as_str())
                );
            }
            WorkerResource::Gcp {
                project_id,
                zone,
                name,
                instance_id,
            } => {
                assert_eq!(inventory["account"].as_str(), Some(project_id.as_str()));
                assert_eq!(inventory["zone"].as_str(), Some(zone.as_str()));
                assert_eq!(inventory["name"].as_str(), Some(name.as_str()));
                let generation = inventory["evidence"]["item"]["instance_id"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                assert_eq!(generation, *instance_id);
            }
            WorkerResource::Azure {
                subscription_id,
                resource_id,
                vm_id,
                ..
            } => {
                assert!(inventory["account"]
                    .as_str()
                    .unwrap()
                    .eq_ignore_ascii_case(subscription_id));
                assert!(inventory["native_reference"]
                    .as_str()
                    .unwrap()
                    .eq_ignore_ascii_case(resource_id));
                assert!(inventory["evidence"]["properties"]["vmId"]
                    .as_str()
                    .unwrap()
                    .eq_ignore_ascii_case(vm_id));
            }
            WorkerResource::Local => {
                panic!("the declared qualification job must run on a cloud worker")
            }
        }
        assert_eq!(row["quote"]["provider"], provider);
        assert_ne!(row["quote"]["source"], "autonomy policy");
        assert!(
            book["quotes"].as_array().unwrap().contains(&row["quote"]),
            "quote was not published by the real producer"
        );
    }
    service.observe("provider_price_book", book);
    service.observe("cloud_allocation_cli", cli);
    service.observe("cloud_allocation_native_api", api);
    assert_eq!(
        service.persisted(),
        before,
        "read-only quotes changed configuration"
    );
    service.pass();
}
