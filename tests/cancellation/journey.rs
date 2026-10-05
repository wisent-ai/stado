//! Live reads of operator-declared, isolated cancellation fixtures. Resources and
//! job records must come from normal Stado lifecycles, never seeded documents.
#[path = "../desktop_api/fixture.rs"]
mod fixture;
mod observations;
use fixture::Service;
use observations::{captured, job, lookup, native_job, scoped};
use serde_json::Value;
use stado::models::WorkerResource;

#[tokio::test]
async fn removed_generation_remains_scoped_in_cli_and_native_report() {
    let mut service = Service::start_with_configuration("STADO_CANCELLATION_QUALIFICATION_CONFIG");
    let id = service.input("STADO_CANCELLATION_REMOVED_JOB_ID");
    let configuration = service.persisted();
    let cli = lookup(&mut service, &id);
    scoped(&cli, true);
    let native = native_job(&mut service, &id).await;
    scoped(&native, true);
    assert_eq!(captured(&cli), captured(&native));
    assert_eq!(service.persisted(), configuration);
    service.pass();
}

#[tokio::test]
async fn repeated_cancellation_preserves_a_stopped_shared_agent_vm() {
    let mut service = Service::start_with_configuration("STADO_CANCELLATION_QUALIFICATION_CONFIG");
    let id = service.input("STADO_CANCELLATION_STOPPED_JOB_ID");
    let configuration = service.persisted();
    let before = lookup(&mut service, &id);
    let resource = scoped(&before, false);
    let state = before["provider_cleanup"]["state"].as_str();
    match resource {
        WorkerResource::Aws { .. } => assert_eq!(
            state,
            Some(aws_sdk_ec2::types::InstanceStateName::Stopped.as_str())
        ),
        WorkerResource::Gcp { .. } => assert_eq!(state, Some("TERMINATED")),
        WorkerResource::Azure { .. } => assert!(matches!(state, Some("stopped" | "deallocated"))),
        WorkerResource::Local => unreachable!(),
    }
    // The declared fixture is already cancelled; this exercises a repeated
    // normal cancellation without creating or deleting a provider resource.
    let repeated = job(
        serde_json::from_str(&service.cli(&["machine", "cancel", &id])).unwrap(),
        &id,
    );
    assert_eq!(repeated["state"], before["state"]);
    let after = native_job(&mut service, &id).await;
    scoped(&after, false);
    assert_eq!(captured(&after), captured(&before));
    assert_eq!(
        after["provider_cleanup"]["state"],
        before["provider_cleanup"]["state"]
    );
    assert_eq!(
        after["provider_cleanup"]["evidence"],
        before["provider_cleanup"]["evidence"]
    );
    assert_eq!(service.persisted(), configuration);
    service.pass();
}

#[tokio::test]
async fn missing_worker_identity_never_becomes_proven_removal() {
    let mut service = Service::start_with_configuration("STADO_CANCELLATION_QUALIFICATION_CONFIG");
    let id = service.input("STADO_CANCELLATION_UNOBSERVED_JOB_ID");
    let configuration = service.persisted();
    let cli = lookup(&mut service, &id);
    let native = native_job(&mut service, &id).await;
    for job in [&cli, &native] {
        captured(job);
        assert_eq!(job["worker_allocation"]["resource"], Value::Null);
        assert!(
            job["worker_allocation"]["error"].is_string(),
            "the actual identity-read failure is required: {job}"
        );
        let observation = &job["provider_cleanup"];
        assert_eq!(observation["removed"], Value::Null);
        assert_eq!(observation["evidence"], Value::Null);
        assert!(
            observation["error"].is_string(),
            "the refusal lost its cause: {observation}"
        );
    }
    assert_eq!(captured(&cli), captured(&native));
    assert_eq!(service.persisted(), configuration);
    service.pass();
}

#[tokio::test]
async fn repeated_cancellation_does_not_delete_a_same_name_replacement() {
    let mut service = Service::start_with_configuration("STADO_CANCELLATION_QUALIFICATION_CONFIG");
    let id = service.input("STADO_CANCELLATION_REPLACED_JOB_ID");
    let configuration = service.persisted();
    let before = lookup(&mut service, &id);
    let resource = scoped(&before, true);
    let evidence = &before["provider_cleanup"]["evidence"];
    match resource {
        WorkerResource::Gcp { instance_id, .. } => {
            let replacement = evidence["instance_id"]
                .as_u64()
                .expect("a real replacement must exist");
            assert_ne!(replacement, instance_id);
        }
        WorkerResource::Azure { vm_id, .. } => {
            let replacement = evidence["vm_id"]
                .as_str()
                .expect("a real replacement must exist");
            assert!(!replacement.eq_ignore_ascii_case(&vm_id));
        }
        _ => panic!("the fixture needs a real same-name GCP or Azure replacement"),
    }
    let repeated = job(
        serde_json::from_str(&service.cli(&["machine", "cancel", &id])).unwrap(),
        &id,
    );
    assert_eq!(repeated["state"], before["state"]);
    let after = native_job(&mut service, &id).await;
    scoped(&after, true);
    assert_eq!(captured(&after), captured(&before));
    assert_eq!(after["provider_cleanup"]["evidence"], *evidence);
    assert_eq!(
        after["provider_cleanup"]["state"],
        before["provider_cleanup"]["state"]
    );
    assert_eq!(service.persisted(), configuration);
    service.pass();
}
