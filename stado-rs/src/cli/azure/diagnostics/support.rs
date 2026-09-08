//! The Azure Support catalog read: which service and which problem
//! classification an RBAC deny belongs under.

use serde_json::Value;

use super::super::{CmdError, ARM_RESOURCE, SUPPORT_API_VERSION};
use super::arm::azure_collection;

pub(super) fn support_display_name(row: &Value) -> &str {
    row.pointer("/properties/displayName")
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn find_support_row<'a>(rows: &'a [Value], alternatives: &[&[&str]]) -> Option<&'a Value> {
    alternatives.iter().find_map(|needles| {
        rows.iter().find(|row| {
            let display = support_display_name(row).to_ascii_lowercase();
            needles
                .iter()
                .all(|needle| display.contains(&needle.to_ascii_lowercase()))
        })
    })
}

pub(super) async fn discover_rbac_support_classification(
    http: &reqwest::Client,
    access_token: &str,
) -> Result<(Value, Value), CmdError> {
    let services = azure_collection(
        http,
        access_token,
        format!(
            "{ARM_RESOURCE}/providers/Microsoft.Support/services?api-version={SUPPORT_API_VERSION}"
        ),
    )
    .await?;
    let service = find_support_row(
        &services,
        &[
            &["role based access control", "azure resources"],
            &["role based access control"],
            &["subscription management"],
        ],
    )
    .cloned()
    .ok_or_else(|| {
        CmdError::click("Azure Support did not return an RBAC or subscription-management service")
    })?;
    let service_name = service
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click("Azure Support RBAC service has no name"))?;
    let classifications = azure_collection(
        http,
        access_token,
        format!(
            "{ARM_RESOURCE}/providers/Microsoft.Support/services/{service_name}/problemClassifications?api-version={SUPPORT_API_VERSION}"
        ),
    )
    .await?;
    let classification = find_support_row(
        &classifications,
        &[
            &["problem", "rbac", "role assignment"],
            &["rbac", "role assignment"],
            &["role assignment"],
            &["permissions"],
        ],
    )
    .cloned()
    .ok_or_else(|| {
        let names = classifications
            .iter()
            .map(support_display_name)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        CmdError::click(format!(
            "Azure Support returned no RBAC role-assignment classification; available: {names}"
        ))
    })?;
    Ok((service, classification))
}
