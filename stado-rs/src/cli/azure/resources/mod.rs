//! `cloud roles repair` — the write verbs against one subscription: the roles
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

/// The value a flag states, else the configured one; empty when neither does.
fn flag_or_configured(flag: Option<&str>, configured: &str) -> String {
    match flag.filter(|value| !value.is_empty()) {
        Some(value) => value.to_string(),
        None => configured.to_string(),
    }
}

pub(in crate::cli::azure) async fn repair_rbac(args: RepairRbacArgs) -> Result<(), CmdError> {
    let subscription = flag_or_configured(
        args.subscription.as_deref(),
        crate::config::azure_subscription_id(),
    );
    if subscription.is_empty() {
        return Err(CmdError::usage(
            "--subscription or AZURE_SUBSCRIPTION_ID is required",
        ));
    }
    let resource_group = flag_or_configured(
        args.resource_group.as_deref(),
        crate::config::azure_resource_group(),
    );
    let configured_storage_account = flag_or_configured(
        args.storage_account.as_deref(),
        crate::config::wc_azure_storage_account(),
    );
    let operator = refresh_operator_token(&args.operator_role).await?;
    let control_principal = control_principal_id(&args).await?;
    let http = reqwest::Client::new();
    // The storage account named, else the one discovery finds in the group;
    // none when the group holds none, and the storage role is then not asked for.
    let storage_account: Option<String> = if configured_storage_account.is_empty() {
        discover_storage_account(
            &http,
            &operator.access_token,
            &subscription,
            &resource_group,
        )
        .await?
    } else {
        Some(configured_storage_account)
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
    let storage_scope = storage_account.as_ref().map(|storage_account| {
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
    let assigned = |role: &Value| role.get("ok").and_then(Value::as_bool) == Some(true);
    let all_assigned = roles.iter().all(assigned);
    let failed = roles.iter().filter(|role| !assigned(role)).count();
    crate::cli::print_answer(
        &json!({
            "operator": {
                "account": operator.account,
                "tenant_id": operator.tenant_id,
                "credential_role": args.operator_role
            },
            "subscription_id": subscription,
            "resource_group": resource_group,
            "storage_account": storage_account,
            "control_principal_id": control_principal,
            "agent_principal_id": agent_principal,
            "roles": roles,
            "failed_role_assignments": failed,
            "deny_assignments": deny_assignments
        }),
        args.json,
    )?;
    if all_assigned {
        Ok(())
    } else {
        Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE))
    }
}
