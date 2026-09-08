//! Deny assignments as `repair-rbac` sees them: reported, and removed only
//! when Azure does not hold them itself.

use serde_json::{json, Value};

use super::super::{CmdError, ARM_RESOURCE, ROLE_API_VERSION};

pub(super) async fn handle_deny_assignments(
    http: &reqwest::Client,
    access_token: &str,
    subscription_scope: &str,
    remove_name: Option<&str>,
) -> Result<Value, CmdError> {
    let response = http
        .get(format!(
            "{ARM_RESOURCE}{subscription_scope}/providers/Microsoft.Authorization/denyAssignments?api-version={ROLE_API_VERSION}"
        ))
        .bearer_auth(access_token)
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Ok(json!({
            "readable": false,
            "http_status": status.as_u16(),
            "error": body.get("error")
        }));
    }
    let mut reports = Vec::new();
    for assignment in body
        .get("value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = assignment.get("name").and_then(Value::as_str).unwrap_or("");
        let display_name = assignment
            .pointer("/properties/denyAssignmentName")
            .or_else(|| assignment.pointer("/properties/name"))
            .and_then(Value::as_str)
            .unwrap_or(name);
        let system_protected = assignment
            .pointer("/properties/isSystemProtected")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let selected = remove_name
            .map(|needle| display_name.contains(needle) || name.contains(needle))
            .unwrap_or(false);
        let mut report = json!({
            "name": name,
            "display_name": display_name,
            "system_protected": system_protected,
            "selected": selected,
            "outcome": "reported"
        });
        if selected && system_protected {
            report["outcome"] = Value::String("microsoft_support_required".into());
        } else if selected {
            let scope = assignment
                .pointer("/properties/scope")
                .and_then(Value::as_str)
                .unwrap_or(subscription_scope);
            let delete = http
                .delete(format!(
                    "{ARM_RESOURCE}{scope}/providers/Microsoft.Authorization/denyAssignments/{name}?api-version={ROLE_API_VERSION}"
                ))
                .bearer_auth(access_token)
                .send()
                .await?;
            report["http_status"] = Value::from(delete.status().as_u16());
            report["outcome"] = Value::String(
                if delete.status().is_success() {
                    "removed"
                } else {
                    "remove_failed"
                }
                .into(),
            );
        }
        reports.push(report);
    }
    Ok(json!({"readable": true, "assignments": reports}))
}
