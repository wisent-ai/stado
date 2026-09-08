//! Probe classification: which detail component owns a successful answer, and
//! how a transport or API failure is recorded instead.

use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::providers::gcp::inventory::compute::addresses::addresses_detail;
use crate::providers::gcp::inventory::compute::disks::disks_detail;
use crate::providers::gcp::inventory::compute::instances::{
    instance_groups_detail, instances_detail, reservations_detail,
};
use crate::providers::gcp::inventory::compute::quotas::region_quota_detail;
use crate::providers::gcp::inventory::fields::compact_plain;
use crate::providers::gcp::inventory::services::cloud_run::{
    cloud_run_iam_policy_detail, cloud_run_revisions_detail, cloud_run_service_detail,
};
use crate::providers::gcp::inventory::services::managed::{
    builds_detail, named_list_detail, scheduler_detail, service_accounts_detail,
};
use crate::providers::gcp::inventory::services::platform::{
    billing_detail, permissions_detail, project_detail, project_iam_policy_detail,
};
use crate::providers::gcp::inventory::ProbeReport;

use super::{ProbeKind, ProbeSpec};

pub(super) fn successful(spec: ProbeSpec, value: Value) -> ProbeReport {
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

pub(super) fn failed_transport(spec: ProbeSpec, error: String) -> ProbeReport {
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

pub(super) fn failed_api(spec: ProbeSpec, status: StatusCode, body: &str) -> ProbeReport {
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
