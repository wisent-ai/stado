//! Deny assignments as `cloud roles repair` sees them: reported, and removed only
//! when Azure does not hold them itself.

use serde_json::{json, Value};

use super::super::{CmdError, ARM_RESOURCE, ROLE_API_VERSION};

pub(super) async fn handle_deny_assignments(
    http: &reqwest::Client,
    access_token: &str,
    subscription_scope: &str,
    remove_name: Option<&str>,
) -> Result<Value, CmdError> {
    let response = crate::wait::request(http
        .get(format!(
            "{ARM_RESOURCE}{subscription_scope}/providers/Microsoft.Authorization/denyAssignments?api-version={ROLE_API_VERSION}"
        ))
        .bearer_auth(access_token)).await?;
    let (status, body) =
        super::discovery::arm_answer(response, "listing Azure deny assignments").await?;
    if !status.is_success() {
        return Ok(json!({
            "readable": false,
            "http_status": status.as_u16(),
            "error": body.get("error")
        }));
    }
    // An answer without its `value` list is not "no deny assignments": it is
    // reported unreadable with the answer, as a refused listing is.
    let Some(assignments) = body.get("value").and_then(Value::as_array) else {
        return Ok(json!({
            "readable": false,
            "http_status": status.as_u16(),
            "error": format!("the deny assignment listing carries no `value` list: {body}"),
        }));
    };
    let mut reports = Vec::new();
    for assignment in assignments {
        // An assignment without its name cannot be addressed, and one whose
        // protection Azure did not state cannot be judged deletable: both
        // are reported with the entry and never deleted.
        let Some(name) = assignment.get("name").and_then(Value::as_str) else {
            reports.push(json!({
                "outcome": "unreadable",
                "error": format!("the deny assignment names no `name`: {assignment}"),
            }));
            continue;
        };
        let display_name = assignment
            .pointer("/properties/denyAssignmentName")
            .or_else(|| assignment.pointer("/properties/name"))
            .and_then(Value::as_str);
        let system_protected = assignment
            .pointer("/properties/isSystemProtected")
            .and_then(Value::as_bool);
        let selected = remove_name.is_some_and(|needle| {
            name.contains(needle) || display_name.is_some_and(|shown| shown.contains(needle))
        });
        let mut report = json!({
            "name": name,
            "display_name": display_name,
            "system_protected": system_protected,
            "selected": selected,
            "outcome": "reported"
        });
        if selected && system_protected == Some(true) {
            report["outcome"] = Value::String("microsoft_support_required".into());
        } else if selected && system_protected.is_none() {
            report["outcome"] = Value::String("protection_unreported".into());
        } else if selected {
            // An assignment is removed at the scope it names; one that names
            // none is a subscription-wide assignment.
            let scope = match assignment
                .pointer("/properties/scope")
                .and_then(Value::as_str)
            {
                Some(scope) => scope,
                None => subscription_scope,
            };
            let delete = crate::wait::request(http
                .delete(format!(
                    "{ARM_RESOURCE}{scope}/providers/Microsoft.Authorization/denyAssignments/{name}?api-version={ROLE_API_VERSION}"
                ))
                .bearer_auth(access_token)).await?;
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
