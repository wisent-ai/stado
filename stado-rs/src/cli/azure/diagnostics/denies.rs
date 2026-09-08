//! The UnusualActivity denies themselves: which inherited, system-protected
//! assignments are active, reported in the shape the ticket text quotes.

use serde_json::{json, Value};

use super::super::{CmdError, ARM_RESOURCE, ROLE_API_VERSION};
use super::arm::azure_collection;

fn deny_display_name(assignment: &Value) -> &str {
    assignment
        .pointer("/properties/denyAssignmentName")
        .or_else(|| assignment.pointer("/properties/name"))
        .and_then(Value::as_str)
        .or_else(|| assignment.get("name").and_then(Value::as_str))
        .unwrap_or("")
}

pub(super) async fn list_unusual_activity_denies(
    http: &reqwest::Client,
    access_token: &str,
    subscription: &str,
) -> Result<Vec<Value>, CmdError> {
    let assignments = azure_collection(
        http,
        access_token,
        format!(
            "{ARM_RESOURCE}/subscriptions/{subscription}/providers/Microsoft.Authorization/denyAssignments?api-version={ROLE_API_VERSION}"
        ),
    )
    .await?;
    Ok(assignments
        .into_iter()
        .filter(|assignment| {
            assignment
                .pointer("/properties/isSystemProtected")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && deny_display_name(assignment)
                    .to_ascii_lowercase()
                    .contains("unusualactivity")
        })
        .map(|assignment| {
            json!({
                "id": assignment.get("id"),
                "name": assignment.get("name"),
                "display_name": deny_display_name(&assignment),
                "scope": assignment.pointer("/properties/scope"),
                "system_protected": assignment.pointer("/properties/isSystemProtected"),
                "applies_to_children": assignment
                    .pointer("/properties/doNotApplyToChildScopes")
                    .and_then(Value::as_bool)
                    .map(|value| !value),
                "principals": assignment.pointer("/properties/principals"),
                "excluded_principals": assignment.pointer("/properties/excludePrincipals"),
                "permissions": assignment.pointer("/properties/permissions"),
                "resolution": "microsoft_support_required"
            })
        })
        .collect())
}
