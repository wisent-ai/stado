//! Role assignments: the deterministic assignment name, the idempotent PUT
//! that applies one role, and the control-plane principal it is applied to.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::super::session::jwt_claims;
use super::super::{parsed, CmdError, RepairRbacArgs, ARM_RESOURCE, ARM_SCOPE, ROLE_API_VERSION};

fn role_assignment_name(scope: &str, principal_id: &str, role_id: &str) -> Uuid {
    let material = format!("{scope}\n{principal_id}\n{role_id}");
    let digest = Sha256::digest(material.as_bytes());
    Uuid::from_slice(&digest[..parsed("16")]).expect("SHA digest prefix is a UUID")
}

pub(super) async fn ensure_role(
    http: &reqwest::Client,
    access_token: &str,
    subscription: &str,
    scope: &str,
    principal_id: &str,
    role_name: &str,
    role_id: &str,
) -> Result<Value, CmdError> {
    let assignment = role_assignment_name(scope, principal_id, role_id);
    let response = http
        .put(format!(
            "{ARM_RESOURCE}{scope}/providers/Microsoft.Authorization/roleAssignments/{assignment}?api-version={ROLE_API_VERSION}"
        ))
        .bearer_auth(access_token)
        .json(&json!({
            "properties": {
                "roleDefinitionId": format!(
                    "/subscriptions/{subscription}/providers/Microsoft.Authorization/roleDefinitions/{role_id}"
                ),
                "principalId": principal_id,
                "principalType": "ServicePrincipal"
            }
        }))
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let error_code = body
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or("");
    let ok = status.is_success() || error_code == "RoleAssignmentExists";
    Ok(json!({
        "role": role_name,
        "scope": scope,
        "principal_id": principal_id,
        "ok": ok,
        "http_status": status.as_u16(),
        "outcome": if status.is_success() { "applied" } else if ok { "already_present" } else { "failed" },
        "error": if ok { Value::Null } else { json!({
            "code": error_code,
            "message": body.pointer("/error/message").and_then(Value::as_str).unwrap_or("Azure role assignment failed")
        }) }
    }))
}

pub(super) async fn control_principal_id(args: &RepairRbacArgs) -> Result<String, CmdError> {
    if let Some(id) = args
        .principal_object_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return Ok(id.to_string());
    }
    let http = reqwest::Client::new();
    let token = crate::azure_token::identity_bearer_token(&http, ARM_SCOPE, ARM_RESOURCE)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    jwt_claims(&token)
        .get("oid")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CmdError::click("stado-azure ARM token has no oid claim"))
}
