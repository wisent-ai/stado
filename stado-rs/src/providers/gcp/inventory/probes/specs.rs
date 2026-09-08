//! The probe set: every GCP surface `stado blast-radius` asks about, and the
//! exact URL it asks on.

use std::collections::BTreeSet;

use serde_json::json;

use crate::providers::gcp::inventory::fields::encode;
use crate::providers::gcp::inventory::InventoryOptions;

use super::requirements::REQUIRED_PERMISSIONS;
use super::{get, post, ProbeKind, ProbeSpec};

const COORDINATOR_SERVICE: &str = "stado-coordinator";
const COORDINATOR_SCHEDULER: &str = "wisent-compute-cron";
const ARTIFACT_REPOSITORY: &str = "stado";

pub(in crate::providers::gcp::inventory) fn probe_specs(
    options: &InventoryOptions,
) -> Vec<ProbeSpec> {
    let project = &options.project;
    let region = &options.region;
    let encoded_project = encode(project);
    let encoded_region = encode(region);
    let mut specs = vec![
        get(
            "project",
            "Cloud Resource Manager",
            project,
            "critical",
            format!("https://cloudresourcemanager.googleapis.com/v3/projects/{encoded_project}"),
            ProbeKind::Project,
        ),
        get(
            "billing_account",
            "Cloud Billing",
            project,
            "critical",
            format!("https://cloudbilling.googleapis.com/v1/projects/{encoded_project}/billingInfo"),
            ProbeKind::Billing,
        ),
        post(
            "caller_permissions",
            "Cloud Resource Manager IAM",
            project,
            "critical",
            format!("https://cloudresourcemanager.googleapis.com/v1/projects/{encoded_project}:testIamPermissions"),
            json!({"permissions": REQUIRED_PERMISSIONS}),
            ProbeKind::IamPermissions,
        ),
        post(
            "service_account_roles",
            "Cloud Resource Manager IAM",
            project,
            "critical",
            format!("https://cloudresourcemanager.googleapis.com/v1/projects/{encoded_project}:getIamPolicy"),
            json!({}),
            ProbeKind::ProjectIamPolicy,
        ),
        get(
            "compute_instances",
            "Compute Engine",
            project,
            "critical",
            format!("https://compute.googleapis.com/compute/v1/projects/{encoded_project}/aggregated/instances?maxResults=500"),
            ProbeKind::Instances,
        ),
        get(
            "compute_disks",
            "Compute Engine",
            project,
            "high",
            format!("https://compute.googleapis.com/compute/v1/projects/{encoded_project}/aggregated/disks?maxResults=500"),
            ProbeKind::Disks,
        ),
        get(
            "managed_instance_groups",
            "Compute Engine",
            project,
            "critical",
            format!("https://compute.googleapis.com/compute/v1/projects/{encoded_project}/aggregated/instanceGroupManagers?maxResults=500"),
            ProbeKind::InstanceGroups,
        ),
        get(
            "compute_reservations",
            "Compute Engine",
            project,
            "medium",
            format!("https://compute.googleapis.com/compute/v1/projects/{encoded_project}/aggregated/reservations?maxResults=500"),
            ProbeKind::Reservations,
        ),
        get(
            "default_network",
            "Compute Engine",
            "global/networks/default",
            "high",
            format!("https://compute.googleapis.com/compute/v1/projects/{encoded_project}/global/networks/default"),
            ProbeKind::Plain,
        ),
        get(
            "firewall_rules",
            "Compute Engine",
            project,
            "medium",
            format!("https://compute.googleapis.com/compute/v1/projects/{encoded_project}/global/firewalls?maxResults=500"),
            ProbeKind::NamedItems,
        ),
        get(
            "compute_snapshots",
            "Compute Engine",
            project,
            "medium",
            format!("https://compute.googleapis.com/compute/v1/projects/{encoded_project}/global/snapshots?maxResults=500"),
            ProbeKind::NamedItems,
        ),
        get(
            "static_addresses",
            "Compute Engine",
            project,
            "medium",
            format!("https://compute.googleapis.com/compute/v1/projects/{encoded_project}/aggregated/addresses?maxResults=500"),
            ProbeKind::Addresses,
        ),
        get(
            "cloud_run_service",
            "Cloud Run",
            &format!("{region}/{COORDINATOR_SERVICE}"),
            "critical",
            format!("https://run.googleapis.com/v2/projects/{encoded_project}/locations/{encoded_region}/services/{COORDINATOR_SERVICE}"),
            ProbeKind::CloudRunService,
        ),
        get(
            "cloud_run_invoker_policy",
            "Cloud Run IAM",
            &format!("{region}/{COORDINATOR_SERVICE}"),
            "critical",
            format!("https://run.googleapis.com/v2/projects/{encoded_project}/locations/{encoded_region}/services/{COORDINATOR_SERVICE}:getIamPolicy"),
            ProbeKind::CloudRunIamPolicy,
        ),
        get(
            "cloud_run_revisions",
            "Cloud Run",
            &format!("{region}/{COORDINATOR_SERVICE}"),
            "high",
            format!("https://run.googleapis.com/v2/projects/{encoded_project}/locations/{encoded_region}/services/{COORDINATOR_SERVICE}/revisions?pageSize=100"),
            ProbeKind::CloudRunRevisions,
        ),
        get(
            "cloud_scheduler",
            "Cloud Scheduler",
            &format!("{region}/{COORDINATOR_SCHEDULER}"),
            "high",
            format!("https://cloudscheduler.googleapis.com/v1/projects/{encoded_project}/locations/{encoded_region}/jobs/{COORDINATOR_SCHEDULER}"),
            ProbeKind::Scheduler,
        ),
        get(
            "cloud_functions",
            "Cloud Functions",
            region,
            "medium",
            format!("https://cloudfunctions.googleapis.com/v2/projects/{encoded_project}/locations/{encoded_region}/functions?pageSize=100"),
            ProbeKind::Functions,
        ),
        get(
            "service_accounts",
            "IAM",
            project,
            "critical",
            format!("https://iam.googleapis.com/v1/projects/{encoded_project}/serviceAccounts?pageSize=100"),
            ProbeKind::ServiceAccounts,
        ),
        get(
            "billing_export_dataset",
            "BigQuery",
            &format!("{}.{}", project, options.billing_dataset),
            "high",
            format!(
                "https://bigquery.googleapis.com/bigquery/v2/projects/{encoded_project}/datasets/{}",
                encode(&options.billing_dataset)
            ),
            ProbeKind::Plain,
        ),
        get(
            "billing_export_table",
            "BigQuery",
            &format!("{}.{}.{}", project, options.billing_dataset, options.billing_table),
            "high",
            format!(
                "https://bigquery.googleapis.com/bigquery/v2/projects/{encoded_project}/datasets/{}/tables/{}",
                encode(&options.billing_dataset),
                encode(&options.billing_table)
            ),
            ProbeKind::Plain,
        ),
        get(
            "artifact_registry",
            "Artifact Registry",
            &format!("{region}/{ARTIFACT_REPOSITORY}"),
            "high",
            format!("https://artifactregistry.googleapis.com/v1/projects/{encoded_project}/locations/{encoded_region}/repositories/{ARTIFACT_REPOSITORY}"),
            ProbeKind::Plain,
        ),
        get(
            "cloud_builds",
            "Cloud Build",
            project,
            "medium",
            format!("https://cloudbuild.googleapis.com/v1/projects/{encoded_project}/builds?pageSize=10"),
            ProbeKind::Builds,
        ),
    ];

    for region_name in &options.regions {
        specs.push(get(
            &format!("compute_region_quota_{region_name}"),
            "Compute Engine",
            region_name,
            "high",
            format!(
                "https://compute.googleapis.com/compute/v1/projects/{encoded_project}/regions/{}",
                encode(region_name),
            ),
            ProbeKind::RegionQuota,
        ));
    }

    if !options.alerts_topic.is_empty() {
        let configured_topic = options.alerts_topic.trim_start_matches('/');
        let topic = if configured_topic.starts_with("projects/") {
            configured_topic.to_string()
        } else {
            format!("projects/{project}/topics/{configured_topic}")
        };
        specs.push(get(
            "pubsub_alert_topic",
            "Pub/Sub",
            &topic,
            "medium",
            format!("https://pubsub.googleapis.com/v1/{topic}"),
            ProbeKind::Plain,
        ));
    }

    let mut seen_buckets = BTreeSet::new();
    for bucket in &options.buckets {
        if bucket.is_empty() || !seen_buckets.insert(bucket.clone()) {
            continue;
        }
        specs.push(get(
            &format!("gcs_bucket_{bucket}"),
            "Cloud Storage",
            &format!("gs://{bucket}"),
            "critical",
            format!(
                "https://storage.googleapis.com/storage/v1/b/{}",
                encode(bucket)
            ),
            ProbeKind::Plain,
        ));
    }
    for object in &options.objects {
        specs.push(get(
            &object.name,
            "Cloud Storage",
            &format!("gs://{}/{}", object.bucket, object.object),
            &object.severity,
            format!(
                "https://storage.googleapis.com/storage/v1/b/{}/o/{}",
                encode(&object.bucket),
                encode(&object.object)
            ),
            ProbeKind::Plain,
        ));
    }
    specs
}
