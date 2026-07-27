//! Read-only, fault-isolated GCP incident inventory used by `stado blast-radius`.
//!
//! Every API is probed independently. A billing or permission failure on one
//! service is data in the report, not an early return that hides the remaining
//! failure domain.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{SecondsFormat, Utc};
use futures::future::join_all;
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde_json::{json, Value};

const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const COORDINATOR_SERVICE: &str = "stado-coordinator";
const COORDINATOR_SCHEDULER: &str = "wisent-compute-cron";
const REQUIRED_SECRET: &str = "wisent-hf-token";
const ARTIFACT_REPOSITORY: &str = "stado";

const REQUIRED_PERMISSIONS: &[&str] = &[
    "artifactregistry.repositories.get",
    "bigquery.datasets.get",
    "bigquery.jobs.create",
    "bigquery.tables.get",
    "bigquery.tables.getData",
    "cloudscheduler.jobs.get",
    "cloudscheduler.jobs.list",
    "cloudfunctions.functions.get",
    "cloudfunctions.functions.list",
    "cloudbuild.builds.list",
    "compute.addresses.list",
    "compute.disks.create",
    "compute.disks.delete",
    "compute.disks.list",
    "compute.firewalls.list",
    "compute.images.get",
    "compute.images.useReadOnly",
    "compute.instanceGroupManagers.list",
    "compute.instances.create",
    "compute.instances.delete",
    "compute.instances.get",
    "compute.instances.setMetadata",
    "compute.instances.list",
    "compute.networks.get",
    "compute.regions.get",
    "compute.reservations.list",
    "compute.snapshots.list",
    "compute.zoneOperations.get",
    "iam.serviceAccounts.actAs",
    "iam.serviceAccounts.get",
    "iam.serviceAccounts.list",
    "pubsub.topics.get",
    "pubsub.topics.publish",
    "resourcemanager.projects.get",
    "resourcemanager.projects.getIamPolicy",
    "run.revisions.list",
    "run.services.get",
    "run.services.getIamPolicy",
    "secretmanager.secrets.get",
    "secretmanager.secrets.list",
    "secretmanager.versions.access",
    "storage.buckets.get",
    "storage.buckets.list",
    "storage.objects.get",
    "storage.objects.create",
    "storage.objects.delete",
    "storage.objects.list",
    "storage.objects.update",
];

const REQUIRED_RUNTIME_ROLES: &[&str] = &[
    "roles/bigquery.dataViewer",
    "roles/bigquery.jobUser",
    "roles/compute.admin",
    "roles/pubsub.publisher",
    "roles/secretmanager.secretAccessor",
    "roles/storage.admin",
];

#[derive(Debug, Clone)]
pub struct GcsObjectAsset {
    pub name: String,
    pub bucket: String,
    pub object: String,
    pub severity: String,
}

#[derive(Debug, Clone)]
pub struct InventoryOptions {
    pub project: String,
    pub region: String,
    pub regions: Vec<String>,
    pub buckets: Vec<String>,
    pub objects: Vec<GcsObjectAsset>,
    pub alerts_topic: String,
    pub billing_dataset: String,
    pub billing_table: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub name: String,
    pub service: String,
    pub resource: String,
    pub severity: String,
    pub state: String,
    pub count: Option<usize>,
    pub detail: Value,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InventorySummary {
    pub state: String,
    pub probes: usize,
    pub healthy: usize,
    pub degraded: usize,
    pub blocked: usize,
    pub missing: usize,
    pub errors: usize,
    pub failed: usize,
    pub critical_failures: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GcpInventoryReport {
    pub checked_at: String,
    pub project: String,
    pub region: String,
    pub summary: InventorySummary,
    pub probes: Vec<ProbeReport>,
}

#[derive(Clone)]
struct Client {
    http: reqwest::Client,
    token: String,
}

#[derive(Clone)]
struct ProbeSpec {
    name: String,
    service: String,
    resource: String,
    severity: String,
    method: Method,
    url: String,
    body: Option<Value>,
    kind: ProbeKind,
}

#[derive(Clone, Copy)]
enum ProbeKind {
    Plain,
    Project,
    Billing,
    IamPermissions,
    ProjectIamPolicy,
    Instances,
    Disks,
    InstanceGroups,
    Reservations,
    RegionQuota,
    NamedItems,
    Addresses,
    CloudRunService,
    CloudRunIamPolicy,
    CloudRunRevisions,
    Scheduler,
    Functions,
    ServiceAccounts,
    Secrets,
    Builds,
}

pub async fn inspect(options: InventoryOptions) -> GcpInventoryReport {
    let provider = match tokio::time::timeout(
        crate::doctor::PROBE_TIMEOUT,
        crate::skarbiec::gcp_provider(),
    )
    .await
    {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => Err(format!(
            "GCP credential resolution exceeded {:?}",
            crate::doctor::PROBE_TIMEOUT
        )),
    };
    let auth = match provider {
        Ok(provider) => {
            match tokio::time::timeout(
                crate::doctor::PROBE_TIMEOUT,
                provider.token(&[CLOUD_PLATFORM_SCOPE]),
            )
            .await
            {
                Ok(Ok(token)) => reqwest::Client::builder()
                    .timeout(crate::doctor::PROBE_TIMEOUT)
                    .build()
                    .map(|http| Client {
                        http,
                        token: token.as_str().to_string(),
                    })
                    .map_err(|error| error.to_string()),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err(format!(
                    "GCP token acquisition exceeded {:?}",
                    crate::doctor::PROBE_TIMEOUT
                )),
            }
        }
        Err(error) => Err(error),
    };

    let mut specs = probe_specs(&options);
    let mut probes = Vec::with_capacity(specs.len().saturating_add(true as usize));
    match auth {
        Ok(client) => {
            probes.push(ProbeReport {
                name: "gcp_runtime_credentials".to_string(),
                service: "Google OAuth".to_string(),
                resource: options.project.clone(),
                severity: "critical".to_string(),
                state: "ok".to_string(),
                count: None,
                detail: json!({
                    "scope": CLOUD_PLATFORM_SCOPE,
                    "token_acquired": true,
                    "source": "platform metadata identity, otherwise stado-gcp in Skarbiec",
                }),
                error: None,
            });
            probes.extend(
                join_all(specs.drain(..).map(|spec| {
                    let client = client.clone();
                    async move { client.run(spec).await }
                }))
                .await,
            );
        }
        Err(error) => {
            probes.push(ProbeReport {
                name: "gcp_runtime_credentials".to_string(),
                service: "Google OAuth".to_string(),
                resource: options.project.clone(),
                severity: "critical".to_string(),
                state: "blocked".to_string(),
                count: None,
                detail: json!({
                    "scope": CLOUD_PLATFORM_SCOPE,
                    "token_acquired": false,
                    "source": "platform metadata identity, otherwise stado-gcp in Skarbiec",
                }),
                error: Some(error.clone()),
            });
            probes.extend(specs.into_iter().map(|spec| ProbeReport {
                name: spec.name,
                service: spec.service,
                resource: spec.resource,
                severity: spec.severity,
                state: "not_checked".to_string(),
                count: None,
                detail: json!({}),
                error: Some(format!("GCP authentication unavailable: {error}")),
            }));
        }
    }

    probes.sort_by(|left, right| left.name.cmp(&right.name));
    let summary = summarize(&probes);
    GcpInventoryReport {
        checked_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        project: options.project,
        region: options.region,
        summary,
        probes,
    }
}

impl Client {
    async fn run(&self, spec: ProbeSpec) -> ProbeReport {
        let mut url = spec.url.clone();
        let mut merged = None;
        let mut seen_tokens = BTreeSet::new();
        loop {
            let mut request = self
                .http
                .request(spec.method.clone(), &url)
                .bearer_auth(&self.token)
                .header(reqwest::header::ACCEPT, "application/json");
            if let Some(body) = &spec.body {
                request = request.json(body);
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => return failed_transport(spec, error.to_string()),
            };
            let status = response.status();
            let body = match response.text().await {
                Ok(body) => body,
                Err(error) => return failed_transport(spec, error.to_string()),
            };
            if !status.is_success() {
                return failed_api(spec, status, &body);
            }
            let value = match serde_json::from_str::<Value>(&body) {
                Ok(value) => value,
                Err(error) => {
                    return failed_transport(spec, format!("invalid JSON response: {error}"))
                }
            };
            let next_token = if spec.method == Method::GET {
                value
                    .get("nextPageToken")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            } else {
                None
            };
            match &mut merged {
                Some(existing) => merge_page(existing, value),
                None => merged = Some(value),
            }
            let Some(token) = next_token else {
                return successful(spec, merged.unwrap_or_else(|| json!({})));
            };
            if !seen_tokens.insert(token.clone()) {
                return failed_transport(spec, format!("pagination token repeated: {token}"));
            }
            let separator = if spec.url.contains('?') { '&' } else { '?' };
            url = format!("{}{separator}pageToken={}", spec.url, encode(&token));
        }
    }
}

fn merge_page(target: &mut Value, page: Value) {
    match (target, page) {
        (Value::Array(target), Value::Array(mut page)) => target.append(&mut page),
        (Value::Object(target), Value::Object(page)) => {
            for (key, value) in page {
                if key == "nextPageToken" {
                    continue;
                }
                match target.get_mut(&key) {
                    Some(existing) => merge_page(existing, value),
                    None => {
                        target.insert(key, value);
                    }
                }
            }
        }
        _ => {}
    }
}

fn probe_specs(options: &InventoryOptions) -> Vec<ProbeSpec> {
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
            "agent_image_family",
            "Compute Engine",
            "global/images/family/wisent-agent",
            "critical",
            format!("https://compute.googleapis.com/compute/v1/projects/{encoded_project}/global/images/family/wisent-agent"),
            ProbeKind::Plain,
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
            "secrets",
            "Secret Manager",
            project,
            "critical",
            format!("https://secretmanager.googleapis.com/v1/projects/{encoded_project}/secrets?pageSize=100"),
            ProbeKind::Secrets,
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

fn get(
    name: &str,
    service: &str,
    resource: &str,
    severity: &str,
    url: String,
    kind: ProbeKind,
) -> ProbeSpec {
    ProbeSpec {
        name: name.to_string(),
        service: service.to_string(),
        resource: resource.to_string(),
        severity: severity.to_string(),
        method: Method::GET,
        url,
        body: None,
        kind,
    }
}

fn post(
    name: &str,
    service: &str,
    resource: &str,
    severity: &str,
    url: String,
    body: Value,
    kind: ProbeKind,
) -> ProbeSpec {
    ProbeSpec {
        name: name.to_string(),
        service: service.to_string(),
        resource: resource.to_string(),
        severity: severity.to_string(),
        method: Method::POST,
        url,
        body: Some(body),
        kind,
    }
}

fn successful(spec: ProbeSpec, value: Value) -> ProbeReport {
    let (state, count, detail) = match spec.kind {
        ProbeKind::Plain => ("ok", None, compact_plain(&value)),
        ProbeKind::Project => project_detail(&value),
        ProbeKind::Billing => billing_detail(&value),
        ProbeKind::IamPermissions => permissions_detail(&value),
        ProbeKind::ProjectIamPolicy => project_iam_policy_detail(&value),
        ProbeKind::Instances => instances_detail(&value),
        ProbeKind::Disks => disks_detail(&value),
        ProbeKind::InstanceGroups => instance_groups_detail(&value),
        ProbeKind::Reservations => reservations_detail(&value),
        ProbeKind::RegionQuota => region_quota_detail(&value),
        ProbeKind::NamedItems => named_list_detail(&value, "items"),
        ProbeKind::Addresses => addresses_detail(&value),
        ProbeKind::CloudRunService => cloud_run_service_detail(&value),
        ProbeKind::CloudRunIamPolicy => cloud_run_iam_policy_detail(&value),
        ProbeKind::CloudRunRevisions => cloud_run_revisions_detail(&value),
        ProbeKind::Scheduler => scheduler_detail(&value),
        ProbeKind::Functions => named_list_detail(&value, "functions"),
        ProbeKind::ServiceAccounts => service_accounts_detail(&value),
        ProbeKind::Secrets => secrets_detail(&value),
        ProbeKind::Builds => builds_detail(&value),
    };
    ProbeReport {
        name: spec.name,
        service: spec.service,
        resource: spec.resource,
        severity: spec.severity,
        state: state.to_string(),
        count,
        detail,
        error: None,
    }
}

fn failed_transport(spec: ProbeSpec, error: String) -> ProbeReport {
    ProbeReport {
        name: spec.name,
        service: spec.service,
        resource: spec.resource,
        severity: spec.severity,
        state: "error".to_string(),
        count: None,
        detail: json!({}),
        error: Some(error),
    }
}

fn failed_api(spec: ProbeSpec, status: StatusCode, body: &str) -> ProbeReport {
    let state = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => "blocked",
        StatusCode::NOT_FOUND => "missing",
        _ => "error",
    };
    ProbeReport {
        name: spec.name,
        service: spec.service,
        resource: spec.resource,
        severity: spec.severity,
        state: state.to_string(),
        count: None,
        detail: json!({"http_status": status.as_u16()}),
        error: Some(api_error(body)),
    }
}

fn api_error(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if let Some(error) = value.get("error") {
            let code = error
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("API_ERROR");
            let message = error.get("message").and_then(Value::as_str).unwrap_or(body);
            return format!("{code}: {}", bounded_error(message));
        }
    }
    bounded_error(body)
}

fn bounded_error(value: &str) -> String {
    value.chars().take(usize::from(u16::MAX)).collect()
}

fn project_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let state = value
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    (
        if state == "ACTIVE" { "ok" } else { "failed" },
        None,
        json!({
            "name": value.get("name"),
            "project_id": value.get("projectId"),
            "project_number": value.get("name").and_then(Value::as_str).and_then(|name| name.rsplit('/').next()),
            "lifecycle_state": state,
            "create_time": value.get("createTime"),
        }),
    )
}

fn billing_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let enabled = value
        .get("billingEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (
        if enabled { "ok" } else { "failed" },
        None,
        json!({
            "billing_enabled": enabled,
            "billing_account": value.get("billingAccountName"),
        }),
    )
}

fn permissions_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let granted: BTreeSet<&str> = value
        .get("permissions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let missing: Vec<&str> = REQUIRED_PERMISSIONS
        .iter()
        .copied()
        .filter(|permission| !granted.contains(permission))
        .collect();
    (
        if missing.is_empty() { "ok" } else { "degraded" },
        Some(granted.len()),
        json!({"granted": granted, "missing": missing, "required": REQUIRED_PERMISSIONS}),
    )
}
fn project_iam_policy_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let mut roles_by_account = BTreeMap::<String, BTreeSet<String>>::new();
    for binding in value
        .get("bindings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(role) = binding.get("role").and_then(Value::as_str) else {
            continue;
        };
        for member in binding
            .get("members")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if member.starts_with("serviceAccount:wisent-compute-sa@")
                || member.starts_with("serviceAccount:stado-sa@")
            {
                roles_by_account
                    .entry(member.to_string())
                    .or_default()
                    .insert(role.to_string());
            }
        }
    }
    let required: BTreeSet<String> = REQUIRED_RUNTIME_ROLES
        .iter()
        .map(|role| (*role).to_string())
        .collect();
    let missing_by_account: BTreeMap<String, BTreeSet<String>> = roles_by_account
        .iter()
        .map(|(account, roles)| {
            (
                account.clone(),
                required.difference(roles).cloned().collect(),
            )
        })
        .collect();
    let complete = missing_by_account.values().any(BTreeSet::is_empty);
    let count = roles_by_account.len();
    (
        if complete { "ok" } else { "degraded" },
        Some(count),
        json!({
            "required_runtime_roles": REQUIRED_RUNTIME_ROLES,
            "roles_by_service_account": roles_by_account,
            "missing_by_service_account": missing_by_account,
            "one_runtime_account_complete": complete,
        }),
    )
}

fn instances_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let mut instances = Vec::new();
    let mut by_status = BTreeMap::<String, usize>::new();
    for item in aggregated(value, "instances") {
        let status = text(item.get("status"));
        *by_status.entry(status.clone()).or_default() += true as usize;
        let accelerators: Vec<Value> = item
            .get("guestAccelerators")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|accelerator| {
                json!({
                    "type": tail(text(accelerator.get("acceleratorType"))),
                    "count": accelerator.get("acceleratorCount"),
                })
            })
            .collect();
        let creator = item
            .get("metadata")
            .and_then(|metadata| metadata.get("items"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|entry| entry.get("key").and_then(Value::as_str) == Some("created-by"))
            .and_then(|entry| entry.get("value"))
            .and_then(Value::as_str)
            .map(tail);
        let disk_gb: u64 = item
            .get("disks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|disk| disk.get("diskSizeGb").and_then(number_u64))
            .sum();
        instances.push(json!({
            "name": item.get("name"),
            "zone": tail(text(item.get("zone"))),
            "status": status,
            "machine_type": tail(text(item.get("machineType"))),
            "accelerators": accelerators,
            "provisioning_model": item.pointer("/scheduling/provisioningModel"),
            "created_by": creator,
            "stado_managed": item.get("name").and_then(Value::as_str).is_some_and(|name| name.starts_with("wisent-agent-")),
            "disk_gb": disk_gb,
            "creation_timestamp": item.get("creationTimestamp"),
        }));
    }
    let count = instances.len();
    (
        "ok",
        Some(count),
        json!({"by_status": by_status, "instances": instances}),
    )
}

fn disks_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let disks: Vec<Value> = aggregated(value, "disks")
        .into_iter()
        .map(|disk| {
            let users = disk
                .get("users")
                .and_then(Value::as_array)
                .map_or(usize::default(), Vec::len);
            json!({
                "name": disk.get("name"),
                "zone": tail(text(disk.get("zone"))),
                "region": tail(text(disk.get("region"))),
                "status": disk.get("status"),
                "size_gb": disk.get("sizeGb"),
                "type": tail(text(disk.get("type"))),
                "users": users,
                "unattached": users == usize::default(),
                "creation_timestamp": disk.get("creationTimestamp"),
            })
        })
        .collect();
    let total_gb: u64 = disks
        .iter()
        .filter_map(|disk| disk.get("size_gb").and_then(number_u64))
        .sum();
    let unattached = disks
        .iter()
        .filter(|disk| disk.get("unattached").and_then(Value::as_bool) == Some(true))
        .count();
    let count = disks.len();
    (
        "ok",
        Some(count),
        json!({"total_gb": total_gb, "unattached": unattached, "disks": disks}),
    )
}

fn instance_groups_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let groups: Vec<Value> = aggregated(value, "instanceGroupManagers")
        .into_iter()
        .map(|group| {
            json!({
                "name": group.get("name"),
                "zone": tail(text(group.get("zone"))),
                "region": tail(text(group.get("region"))),
                "target_size": group.get("targetSize"),
                "instance_template": tail(text(group.get("instanceTemplate"))),
                "stable": group.pointer("/status/isStable"),
                "version_target_reached": group.pointer("/status/versionTarget/isReached"),
                "creation_timestamp": group.get("creationTimestamp"),
            })
        })
        .collect();
    let target_instances: u64 = groups
        .iter()
        .filter_map(|group| group.get("target_size").and_then(number_u64))
        .sum();
    let count = groups.len();
    (
        "ok",
        Some(count),
        json!({"target_instances": target_instances, "managed_instance_groups": groups}),
    )
}

fn reservations_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let reservations: Vec<Value> = aggregated(value, "reservations")
        .into_iter()
        .map(|reservation| {
            json!({
                "name": reservation.get("name"),
                "zone": tail(text(reservation.get("zone"))),
                "status": reservation.get("status"),
                "specific_reservation": reservation.get("specificReservation"),
                "specific_reservation_required": reservation.get("specificReservationRequired"),
                "creation_timestamp": reservation.get("creationTimestamp"),
            })
        })
        .collect();
    let count = reservations.len();
    ("ok", Some(count), json!({"reservations": reservations}))
}
fn region_quota_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let quotas: Vec<Value> = value
        .get("quotas")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|quota| {
            let metric = quota
                .get("metric")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN");
            let usage = quota
                .get("usage")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            let limit = quota
                .get("limit")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            json!({
                "metric": metric,
                "usage": usage,
                "limit": limit,
                "exhausted": limit > f64::default() && usage >= limit,
            })
        })
        .collect();
    let exhausted: Vec<&Value> = quotas
        .iter()
        .filter(|quota| quota.get("exhausted").and_then(Value::as_bool) == Some(true))
        .collect();
    let count = quotas.len();
    (
        if exhausted.is_empty() {
            "ok"
        } else {
            "degraded"
        },
        Some(count),
        json!({
            "region": value.get("name"),
            "status": value.get("status"),
            "exhausted": exhausted,
            "quotas": quotas,
        }),
    )
}

fn addresses_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let addresses: Vec<Value> = aggregated(value, "addresses")
        .into_iter()
        .map(|address| {
            json!({
                "name": address.get("name"),
                "region": tail(text(address.get("region"))),
                "status": address.get("status"),
                "address_type": address.get("addressType"),
                "ip_version": address.get("ipVersion"),
                "purpose": address.get("purpose"),
                "users": address.get("users"),
                "creation_timestamp": address.get("creationTimestamp"),
            })
        })
        .collect();
    let count = addresses.len();
    ("ok", Some(count), json!({"addresses": addresses}))
}

fn cloud_run_service_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let terminal = value
        .pointer("/terminalCondition/state")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    let latest_ready = value.get("latestReadyRevision").and_then(Value::as_str);
    let latest_created = value.get("latestCreatedRevision").and_then(Value::as_str);
    let ready = terminal == "CONDITION_SUCCEEDED"
        && latest_ready.is_some()
        && latest_ready == latest_created;
    let mut environment = BTreeMap::new();
    for container in value
        .pointer("/template/containers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for variable in container
            .get("env")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = variable.get("name").and_then(Value::as_str) else {
                continue;
            };
            if matches!(
                name,
                "GCP_PROJECT"
                    | "GOOGLE_CLOUD_PROJECT"
                    | "WC_BUCKET"
                    | "WC_ALERTS_TOPIC"
                    | "WC_COORDINATOR_ID"
                    | "STADO_DEPLOYMENT_ID"
                    | "WC_RELEASE_BASE_URL"
                    | "WC_PROVIDERS"
                    | "WC_STORAGE_BACKEND"
            ) {
                environment.insert(name, variable.get("value").and_then(Value::as_str));
            }
        }
    }
    (
        if ready { "ok" } else { "degraded" },
        Some(true as usize),
        json!({
            "name": value.get("name"),
            "uri": value.get("uri"),
            "terminal_condition": value.get("terminalCondition"),
            "latest_ready_revision": latest_ready,
            "latest_created_revision": latest_created,
            "service_account": value.pointer("/template/serviceAccount"),
            "environment": environment,
            "update_time": value.get("updateTime"),
        }),
    )
}
fn cloud_run_iam_policy_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let invokers: BTreeSet<&str> = value
        .get("bindings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|binding| binding.get("role").and_then(Value::as_str) == Some("roles/run.invoker"))
        .flat_map(|binding| {
            binding
                .get("members")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        })
        .collect();
    let authenticated_invoker = invokers
        .iter()
        .any(|member| member.starts_with("serviceAccount:"));
    let publicly_invokable = invokers.contains("allUsers");
    let state = if authenticated_invoker && !publicly_invokable {
        "ok"
    } else {
        "degraded"
    };
    (
        state,
        Some(invokers.len()),
        json!({
            "invokers": invokers,
            "authenticated_service_account_invoker": authenticated_invoker,
            "publicly_invokable": publicly_invokable,
            "etag": value.get("etag"),
        }),
    )
}

fn cloud_run_revisions_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let revisions: Vec<Value> = value
        .get("revisions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|revision| {
            json!({
                "name": revision.get("name"),
                "terminal_condition": revision.get("terminalCondition"),
                "service_account": revision.get("serviceAccount"),
                "create_time": revision.get("createTime"),
            })
        })
        .collect();
    let unhealthy = revisions
        .iter()
        .filter(|revision| {
            revision
                .pointer("/terminal_condition/state")
                .and_then(Value::as_str)
                != Some("CONDITION_SUCCEEDED")
        })
        .count();
    let count = revisions.len();
    (
        if unhealthy == usize::default() {
            "ok"
        } else {
            "degraded"
        },
        Some(count),
        json!({"unhealthy": unhealthy, "revisions": revisions}),
    )
}

fn scheduler_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let state = value
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    (
        if state == "ENABLED" { "ok" } else { "degraded" },
        Some(true as usize),
        json!({
            "name": value.get("name"),
            "state": state,
            "schedule": value.get("schedule"),
            "time_zone": value.get("timeZone"),
            "http_target": {
                "uri": value.pointer("/httpTarget/uri"),
                "method": value.pointer("/httpTarget/httpMethod"),
                "oidc_service_account": value.pointer("/httpTarget/oidcToken/serviceAccountEmail"),
                "oidc_audience": value.pointer("/httpTarget/oidcToken/audience"),
                "oauth_service_account": value.pointer("/httpTarget/oauthToken/serviceAccountEmail"),
                "oauth_scope": value.pointer("/httpTarget/oauthToken/scope"),
            },
            "last_attempt_time": value.get("lastAttemptTime"),
            "status": value.get("status"),
        }),
    )
}

fn named_list_detail(value: &Value, key: &str) -> (&'static str, Option<usize>, Value) {
    let names: Vec<&str> = value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .collect();
    ("ok", Some(names.len()), json!({"names": names}))
}

fn service_accounts_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let accounts: Vec<Value> = value
        .get("accounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|account| {
            json!({
                "email": account.get("email"),
                "disabled": account.get("disabled"),
                "display_name": account.get("displayName"),
            })
        })
        .collect();
    let required_present = accounts.iter().any(|account| {
        account
            .get("email")
            .and_then(Value::as_str)
            .is_some_and(|email| email.starts_with("wisent-compute-sa@"))
            && account.get("disabled").and_then(Value::as_bool) != Some(true)
    });
    let count = accounts.len();
    (
        if required_present { "ok" } else { "degraded" },
        Some(count),
        json!({"required_service_account_present_and_enabled": required_present, "accounts": accounts}),
    )
}

fn secrets_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let names: Vec<String> = value
        .get("secrets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|secret| secret.get("name").and_then(Value::as_str).map(tail))
        .collect();
    let required_present = names.iter().any(|name| name == REQUIRED_SECRET);
    let count = names.len();
    (
        if required_present { "ok" } else { "degraded" },
        Some(count),
        json!({"required": REQUIRED_SECRET, "required_present": required_present, "names": names}),
    )
}

fn builds_detail(value: &Value) -> (&'static str, Option<usize>, Value) {
    let builds: Vec<Value> = value
        .get("builds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|build| {
            json!({
                "id": build.get("id"),
                "status": build.get("status"),
                "create_time": build.get("createTime"),
                "finish_time": build.get("finishTime"),
                "images": build.get("images"),
                "log_url": build.get("logUrl"),
            })
        })
        .collect();
    let latest_failed = builds
        .first()
        .and_then(|build| build.get("status"))
        .and_then(Value::as_str)
        .is_some_and(|status| {
            matches!(
                status,
                "FAILURE" | "INTERNAL_ERROR" | "TIMEOUT" | "CANCELLED" | "EXPIRED"
            )
        });
    let count = builds.len();
    (
        if latest_failed { "degraded" } else { "ok" },
        Some(count),
        json!({"builds": builds}),
    )
}

fn compact_plain(value: &Value) -> Value {
    let mut detail = serde_json::Map::new();
    for key in [
        "name",
        "id",
        "state",
        "status",
        "location",
        "storageClass",
        "timeCreated",
        "updated",
        "createTime",
        "updateTime",
        "format",
        "kmsKeyName",
        "numBytes",
        "generation",
        "metageneration",
        "etag",
    ] {
        if let Some(entry) = value.get(key) {
            detail.insert(key.to_string(), entry.clone());
        }
    }
    Value::Object(detail)
}

fn aggregated<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    value
        .get("items")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|items| items.values())
        .filter_map(|scope| scope.get(key).and_then(Value::as_array))
        .flatten()
        .collect()
}

fn text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn tail(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    value.rsplit('/').next().unwrap_or(value).to_string()
}

fn number_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

fn encode(value: &str) -> String {
    crate::queue::gcs::percent_encode(value)
}

fn summarize(probes: &[ProbeReport]) -> InventorySummary {
    let healthy = probes.iter().filter(|probe| probe.state == "ok").count();
    let degraded = probes
        .iter()
        .filter(|probe| probe.state == "degraded")
        .count();
    let blocked = probes
        .iter()
        .filter(|probe| matches!(probe.state.as_str(), "blocked" | "not_checked"))
        .count();
    let missing = probes
        .iter()
        .filter(|probe| probe.state == "missing")
        .count();
    let errors = probes.iter().filter(|probe| probe.state == "error").count();
    let failed = probes
        .iter()
        .filter(|probe| probe.state == "failed")
        .count();
    let critical_failures = probes
        .iter()
        .filter(|probe| probe.severity == "critical" && !matches!(probe.state.as_str(), "ok"))
        .count();
    let state = if critical_failures != usize::default() {
        "critical"
    } else if degraded + blocked + missing + errors + failed != usize::default() {
        "degraded"
    } else {
        "healthy"
    };
    InventorySummary {
        state: state.to_string(),
        probes: probes.len(),
        healthy,
        degraded,
        blocked,
        missing,
        errors,
        failed,
        critical_failures,
    }
}
