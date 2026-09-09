//! Operator selectors: `gcp:TYPE:LOCATION/NAME` validated and turned into one
//! draft action, plus the component checks every name and location must pass.

use serde_json::json;
use uuid::Uuid;

use crate::cli::resources::model::{
    Action, ActionKind, Authorization, ProviderKind, ResourceLocator, Reversibility,
};
use crate::cli::CmdError;

pub(super) fn parse_selector(project: &str, selector: &str) -> Result<Action, CmdError> {
    let mut parts = selector.splitn("gcp".len(), ':');
    let provider = parts.next().unwrap_or_default();
    let resource_type = parts.next().unwrap_or_default();
    let locator = parts.next().unwrap_or_default();
    if !crate::capabilities::ProviderId::Gcp.matches(provider)
        || resource_type.is_empty()
        || locator.is_empty()
    {
        return Err(CmdError::usage(format!(
            "resource {selector:?} must use gcp:TYPE:LOCATION/NAME"
        )));
    }
    let (location, name) = if resource_type == "cloud-sql" {
        (None, locator)
    } else {
        let (location, name) = locator
            .split_once('/')
            .ok_or_else(|| CmdError::usage(format!("resource {selector:?} needs LOCATION/NAME")))?;
        (Some(location.to_string()), name)
    };
    validate_component(name, "resource name")?;
    if let Some(location) = location.as_deref() {
        validate_component(location, "resource location")?;
    }
    let (kind, scope, normalized_type) = match resource_type {
        "scheduler" => (ActionKind::PauseScheduler, "region", "scheduler-job"),
        "zonal-mig" => (
            ActionKind::ResizeManagedInstanceGroup,
            "zone",
            "managed-instance-group",
        ),
        "regional-mig" => (
            ActionKind::ResizeManagedInstanceGroup,
            "region",
            "managed-instance-group",
        ),
        "instance" => (ActionKind::StopInstance, "zone", "instance"),
        "cloud-sql" => (ActionKind::SuspendCloudSql, "global", "cloud-sql-instance"),
        other => {
            return Err(CmdError::usage(format!(
                "unsupported shutdown resource type {other:?}"
            )))
        }
    };
    Ok(Action {
        id: format!("action-{}", Uuid::new_v4().simple()),
        finding_id: None,
        kind,
        authorization: Authorization::Automatic,
        reversibility: Reversibility::Reversible,
        resource: ResourceLocator {
            provider: ProviderKind::Gcp,
            resource_type: normalized_type.to_string(),
            project: Some(project.to_string()),
            location,
            name: name.to_string(),
            reference: selector.to_string(),
        },
        parameters: json!({"scope": scope}),
        preconditions: Vec::new(),
        postconditions: Vec::new(),
        rollback: None,
        depends_on: Vec::new(),
    })
}

pub(super) fn validate_project(project: &str) -> Result<(), CmdError> {
    validate_component(project, "GCP project")?;
    if project.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CmdError::usage(
            "GCP project must be an explicit project id, not a numeric project number",
        ));
    }
    Ok(())
}

fn validate_component(value: &str, label: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(CmdError::usage(format!("invalid {label} {value:?}")));
    }
    Ok(())
}
