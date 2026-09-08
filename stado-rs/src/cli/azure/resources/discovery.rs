//! Reading the resource group: which storage account the queue uses, and
//! which managed identity the agent VMs run as.

use serde_json::Value;

use super::super::{one, CmdError, ARM_RESOURCE, IDENTITY_API_VERSION, STORAGE_API_VERSION};

async fn list_resource_collection(
    http: &reqwest::Client,
    access_token: &str,
    subscription: &str,
    resource_group: &str,
    provider_path: &str,
    api_version: &str,
) -> Result<Vec<Value>, CmdError> {
    let response = http
        .get(format!(
            "{ARM_RESOURCE}/subscriptions/{subscription}/resourceGroups/{resource_group}/providers/{provider_path}?api-version={api_version}"
        ))
        .bearer_auth(access_token)
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(CmdError::click(format!(
            "cannot list Azure {provider_path} with HTTP {status}: {}",
            body.pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("unknown ARM error")
        )));
    }
    Ok(body
        .get("value")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn select_resource<'a>(resources: &'a [Value], preferred_prefix: &str) -> Option<&'a Value> {
    let matching: Vec<&Value> = resources
        .iter()
        .filter(|resource| {
            resource
                .get("name")
                .and_then(Value::as_str)
                .map(|name| name.starts_with(preferred_prefix))
                .unwrap_or(false)
        })
        .collect();
    if matching.len() == one() {
        matching.first().copied()
    } else if resources.len() == one() {
        resources.first()
    } else {
        None
    }
}

pub(super) async fn discover_storage_account(
    http: &reqwest::Client,
    access_token: &str,
    subscription: &str,
    resource_group: &str,
) -> Result<Option<String>, CmdError> {
    let resources = list_resource_collection(
        http,
        access_token,
        subscription,
        resource_group,
        "Microsoft.Storage/storageAccounts",
        STORAGE_API_VERSION,
    )
    .await?;
    Ok(select_resource(&resources, "stado")
        .and_then(|resource| resource.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string))
}

pub(super) async fn agent_principal_id(
    http: &reqwest::Client,
    access_token: &str,
    subscription: &str,
    resource_group: &str,
    explicit: Option<&str>,
) -> Result<Option<String>, CmdError> {
    if let Some(id) = explicit.filter(|value| !value.is_empty()) {
        return Ok(Some(id.to_string()));
    }
    let configured_resource_id = crate::config::azure_vm_identity_id();
    let resource = if configured_resource_id.is_empty() {
        let resources = list_resource_collection(
            http,
            access_token,
            subscription,
            resource_group,
            "Microsoft.ManagedIdentity/userAssignedIdentities",
            IDENTITY_API_VERSION,
        )
        .await?;
        select_resource(&resources, "stado-agent").cloned()
    } else {
        let response = http
            .get(format!(
                "{ARM_RESOURCE}{configured_resource_id}?api-version={IDENTITY_API_VERSION}"
            ))
            .bearer_auth(access_token)
            .send()
            .await?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(CmdError::click(format!(
                "cannot resolve Azure VM identity with HTTP {status}: {}",
                body.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown ARM error")
            )));
        }
        Some(body)
    };
    Ok(resource
        .as_ref()
        .and_then(|value| value.pointer("/properties/principalId"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string))
}
