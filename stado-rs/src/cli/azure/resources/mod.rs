//! `azure repair-rbac` — the write verbs against one subscription: the roles
//! the control plane and the agent identity need, and the deny assignments
//! that may be standing in their way.

use serde_json::{json, Value};

use super::session::refresh_operator_token;
use super::{
    CmdError, RepairRbacArgs, CONTRIBUTOR_ROLE, QUOTA_REQUEST_OPERATOR_ROLE,
    STORAGE_BLOB_DATA_CONTRIBUTOR_ROLE, SUPPORT_REQUEST_CONTRIBUTOR_ROLE,
    VIRTUAL_MACHINE_CONTRIBUTOR_ROLE,
};

mod denies;
mod discovery;
mod roles;

use denies::handle_deny_assignments;
use discovery::{agent_principal_id, discover_storage_account};
use roles::{control_principal_id, ensure_role};

pub(in crate::cli::azure) async fn repair_rbac(args: RepairRbacArgs) -> Result<(), CmdError> {
    let subscription = args
        .subscription
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::config::azure_subscription_id().to_string());
    if subscription.is_empty() {
        return Err(CmdError::usage(
            "--subscription or AZURE_SUBSCRIPTION_ID is required",
        ));
    }
    let resource_group = args
        .resource_group
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::config::azure_resource_group().to_string());
    let configured_storage_account = args
        .storage_account
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::config::wc_azure_storage_account().to_string());
    let operator = refresh_operator_token(&args.operator_item).await?;
    let control_principal = control_principal_id(&args).await?;
    let http = reqwest::Client::new();
    let storage_account = if configured_storage_account.is_empty() {
        discover_storage_account(
            &http,
            &operator.access_token,
            &subscription,
            &resource_group,
        )
        .await?
        .unwrap_or_default()
    } else {
        configured_storage_account
    };
    let agent_principal = agent_principal_id(
        &http,
        &operator.access_token,
        &subscription,
        &resource_group,
        args.agent_object_id.as_deref(),
    )
    .await?;
    let subscription_scope = format!("/subscriptions/{subscription}");
    let group_scope = format!("{subscription_scope}/resourceGroups/{resource_group}");
    let storage_scope = (!storage_account.is_empty()).then(|| {
        format!("{group_scope}/providers/Microsoft.Storage/storageAccounts/{storage_account}")
    });

    let mut roles = Vec::new();
    roles.push(
        ensure_role(
            &http,
            &operator.access_token,
            &subscription,
            &group_scope,
            &control_principal,
            "Contributor",
            CONTRIBUTOR_ROLE,
        )
        .await?,
    );
    for (role_name, role_id) in [
        ("Quota Request Operator", QUOTA_REQUEST_OPERATOR_ROLE),
        (
            "Support Request Contributor",
            SUPPORT_REQUEST_CONTRIBUTOR_ROLE,
        ),
    ] {
        roles.push(
            ensure_role(
                &http,
                &operator.access_token,
                &subscription,
                &subscription_scope,
                &control_principal,
                role_name,
                role_id,
            )
            .await?,
        );
    }
    if let Some(scope) = storage_scope.as_deref() {
        roles.push(
            ensure_role(
                &http,
                &operator.access_token,
                &subscription,
                scope,
                &control_principal,
                "Storage Blob Data Contributor",
                STORAGE_BLOB_DATA_CONTRIBUTOR_ROLE,
            )
            .await?,
        );
    }
    if let Some(principal) = agent_principal.as_deref() {
        roles.push(
            ensure_role(
                &http,
                &operator.access_token,
                &subscription,
                &group_scope,
                principal,
                "Virtual Machine Contributor",
                VIRTUAL_MACHINE_CONTRIBUTOR_ROLE,
            )
            .await?,
        );
        if let Some(scope) = storage_scope.as_deref() {
            roles.push(
                ensure_role(
                    &http,
                    &operator.access_token,
                    &subscription,
                    scope,
                    principal,
                    "Storage Blob Data Contributor",
                    STORAGE_BLOB_DATA_CONTRIBUTOR_ROLE,
                )
                .await?,
            );
        }
    }
    let deny_assignments = handle_deny_assignments(
        &http,
        &operator.access_token,
        &subscription_scope,
        args.remove_deny_name.as_deref(),
    )
    .await?;
    let failed = roles
        .iter()
        .filter(|role| role.get("ok").and_then(Value::as_bool) != Some(true))
        .count();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "operator": {
                "account": operator.account,
                "tenant_id": operator.tenant_id,
                "credential": args.operator_item
            },
            "subscription_id": subscription,
            "resource_group": resource_group,
            "storage_account": if storage_account.is_empty() { Value::Null } else { Value::String(storage_account.to_string()) },
            "control_principal_id": control_principal,
            "agent_principal_id": agent_principal,
            "roles": roles,
            "failed_role_assignments": failed,
            "deny_assignments": deny_assignments
        }))?
    );
    if failed == usize::default() {
        Ok(())
    } else {
        Err(CmdError::silent(i32::from(true)))
    }
}
