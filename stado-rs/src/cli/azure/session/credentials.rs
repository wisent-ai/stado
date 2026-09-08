//! The credential-store reads and the one write: where the operator refresh
//! token lives, and the named fields it is read back out of.

use serde_json::{json, Value};

use super::super::{CmdError, ARM_SCOPE, AZURE_CLI_CLIENT_ID};

fn credential_client() -> Result<crate::skarbiec::Client, CmdError> {
    let credentials = crate::credential_store::admin_credentials()
        .map_err(|error| CmdError::click(error.to_string()))?;
    crate::skarbiec::Client::direct(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    )
    .map_err(|error| CmdError::click(error.to_string()))
}

/// One named field of a credential item. The whole-item form is refused by a
/// broker that requires a named field, and every caller here knows the field
/// it wants.
pub(super) async fn credential_field(id: &str, field: &str) -> Result<Option<String>, CmdError> {
    credential_client()?
        .read_string(id, field)
        .await
        .map_err(|error| CmdError::click(format!("cannot read credential item {id}: {error}")))
}

pub(super) async fn store_operator_item(
    id: &str,
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
    credential_client()?
        .write_item(id, "stado-secret", &value)
        .await
        .map_err(|error| {
            CmdError::click(format!("cannot store Azure operator credential: {error}"))
        })
}
