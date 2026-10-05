use super::fixture::Service;
use serde_json::{json, Value};
use stado::models::WorkerResource;

pub(super) fn job(envelope: Value, id: &str) -> Value {
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["schema_version"], 1);
    let job = envelope["result"]["job"].clone();
    assert_eq!(job["job_id"], id);
    assert_eq!(
        job["terminal"], true,
        "fixture must already be terminal: {job}"
    );
    job
}

pub(super) fn lookup(service: &mut Service, id: &str) -> Value {
    job(
        serde_json::from_str(&service.cli(&["machine", "status", id])).unwrap(),
        id,
    )
}

pub(super) async fn native_job(service: &mut Service, id: &str) -> Value {
    let receipt = service
        .call(json!({"args": ["cost", "quote", "--json", "--", id]}), 200)
        .await;
    assert_eq!(receipt["ok"], true, "{receipt}");
    assert_eq!(receipt["exit_code"], 0, "{receipt}");
    assert_eq!(receipt["stdout_truncated"], false);
    assert_eq!(receipt["stderr_truncated"], false);
    let report: Value = serde_json::from_str(receipt["stdout"].as_str().unwrap()).unwrap();
    let row = report["quotes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["job_id"] == id)
        .unwrap();
    let job = row["allocation"]["job"].clone();
    assert_eq!(
        job["job_id"], id,
        "the native report lost the terminal allocation: {report}"
    );
    job
}

pub(super) fn captured(job: &Value) -> &Value {
    let observation = &job["provider_cleanup"];
    assert_eq!(observation["job_id"], job["job_id"]);
    assert_eq!(observation["operation"], "observe_instance_removal");
    let allocation = &observation["allocation"];
    assert_eq!(allocation["started_at"], job["started_at"]);
    assert!(
        job["started_at"].is_string(),
        "fixture must have actually executed"
    );
    assert_eq!(allocation["worker_allocation"], job["worker_allocation"]);
    assert_eq!(
        allocation["instance_ref"]
            .as_str()
            .unwrap()
            .strip_prefix("local@"),
        job["worker_allocation"]["host"].as_str()
    );
    let captured_restarts = allocation["restarts"].as_i64().unwrap();
    assert!(captured_restarts >= 0 && captured_restarts <= job["restarts"].as_i64().unwrap());
    let captured_at =
        chrono::DateTime::parse_from_rfc3339(allocation["captured_at"].as_str().unwrap()).unwrap();
    let observed_at =
        chrono::DateTime::parse_from_rfc3339(observation["observed_at"].as_str().unwrap()).unwrap();
    assert!(observed_at >= captured_at);
    allocation
}

pub(super) fn scoped(job: &Value, removed: bool) -> WorkerResource {
    captured(job);
    let observation = &job["provider_cleanup"];
    assert_eq!(
        observation["error"],
        Value::Null,
        "real provider observation failed: {observation}"
    );
    assert_eq!(observation["removed"], removed, "{observation}");
    assert_eq!(job["worker_allocation"]["error"], Value::Null);
    let resource: WorkerResource =
        serde_json::from_value(job["worker_allocation"]["resource"].clone()).unwrap();
    let evidence = &observation["evidence"];
    match &resource {
        WorkerResource::Aws {
            account_id,
            region,
            instance_id,
        } => {
            assert_eq!(evidence["operation"], "EC2.DescribeInstances");
            assert_eq!(evidence["account_id"].as_str(), Some(account_id.as_str()));
            assert_eq!(evidence["region"].as_str(), Some(region.as_str()));
            assert_eq!(evidence["instance_id"].as_str(), Some(instance_id.as_str()));
            let state = observation["state"].as_str();
            assert_eq!(
                removed,
                state.is_none()
                    || state == Some(aws_sdk_ec2::types::InstanceStateName::Terminated.as_str())
            );
        }
        WorkerResource::Gcp {
            project_id,
            zone,
            name,
            instance_id,
        } => {
            assert_eq!(evidence["operation"], "compute.instances.get");
            assert_eq!(evidence["project_id"].as_str(), Some(project_id.as_str()));
            assert_eq!(evidence["zone"].as_str(), Some(zone.as_str()));
            assert_eq!(evidence["name"].as_str(), Some(name.as_str()));
            assert_eq!(
                removed,
                evidence["instance_id"].as_u64() != Some(*instance_id)
            );
        }
        WorkerResource::Azure {
            subscription_id,
            resource_id,
            vm_id,
            ..
        } => {
            assert_eq!(
                evidence["operation"],
                "Microsoft.Compute.virtualMachines.get"
            );
            assert!(evidence["subscription_id"]
                .as_str()
                .unwrap()
                .eq_ignore_ascii_case(subscription_id));
            assert!(evidence["resource_id"]
                .as_str()
                .unwrap()
                .eq_ignore_ascii_case(resource_id));
            assert_eq!(
                removed,
                evidence["vm_id"]
                    .as_str()
                    .is_none_or(|observed| !observed.eq_ignore_ascii_case(vm_id))
            );
        }
        WorkerResource::Local => panic!("a local policy cannot qualify physical cloud removal"),
    }
    resource
}
