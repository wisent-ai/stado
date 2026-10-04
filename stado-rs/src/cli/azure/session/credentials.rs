//! The credential-store reads and the one write: where the operator refresh
//! token lives, and the named fields it is read back out of.

use serde_json::{json, Value};

use super::super::{CmdError, ARM_SCOPE, AZURE_CLI_CLIENT_ID};

fn credential_client() -> Result<crate::skarbiec::Client, CmdError> {
    let credentials = crate::credential_store::admin_credentials().map_err(CmdError::from)?;
    crate::skarbiec::Client::direct(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    )
    .map_err(CmdError::from)
}

/// One named field of the credential item that plays `role`. The whole-item
/// form is refused by a broker that requires a named field, and every caller
/// here knows the field it wants.
pub(super) async fn credential_field(role: &str, field: &str) -> Result<Option<String>, CmdError> {
    credential_client()?
        .read_string(role, field)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "cannot read the credential playing role {role}: {error}"
            ))
            .stating(error.failure_code())
        })
}

/// Store the operator session in the item that plays `role`, the same role
/// [`credential_field`] reads it back by, so login and every later read agree
/// on the item without either naming it.
pub(super) async fn store_operator_item(
    role: &str,
    tenant: &str,
    account: &str,
    refresh_token: &str,
    token_body: &Value,
) -> Result<(), CmdError> {
    let value = json!({
        "display_name": "Stado Azure operator session",
        "login_email": account,
        "tenant_id": tenant,
        "client_id": AZURE_CLI_CLIENT_ID,
        "refresh_token": refresh_token,
        "scope": token_body.get("scope").and_then(Value::as_str).unwrap_or(ARM_SCOPE),
        "client_info": token_body.get("client_info").and_then(Value::as_str).unwrap_or(""),
        "credential_status": "ready",
        "tags": ["wisent", "azure", "operator", "oauth-refresh"]
    });
    // `stado-secret` is the canonical kind that carries arbitrary named fields.
    // `oauth-client` is not it: that kind allows only a client id and secret, so
    // a session with a refresh token, tenant and scope is refused by the schema.
    crate::credential_store::write::write_role_item_with(role, "stado-secret", &value, &json!({}))
        .await
        .map(|_| ())
        .map_err(|error| {
            CmdError::click(format!("cannot store Azure operator credential: {error}"))
                .stating(error.failure_code())
        })
}
