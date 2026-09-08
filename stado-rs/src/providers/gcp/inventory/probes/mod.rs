//! One probe: what it is called, what it asks for, and how its answer is
//! classified. Construction lives here; execution and classification are the
//! sibling components.

pub(super) mod client;
mod outcome;
pub(super) mod requirements;
pub(super) mod specs;

use reqwest::Method;
use serde_json::Value;

#[derive(Clone)]
pub(super) struct ProbeSpec {
    pub(super) name: String,
    pub(super) service: String,
    pub(super) resource: String,
    pub(super) severity: String,
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
    Builds,
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
