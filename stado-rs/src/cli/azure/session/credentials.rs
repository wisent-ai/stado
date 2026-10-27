//! The credential-store reads and the one write: where the operator refresh
//! token lives, and the named fields it is read back out of.

use serde_json::{json, Value};

use super::super::{CmdError, AZURE_CLI_CLIENT_ID};

/// One named field of the credential item that plays `role`, read through the
/// selected credential store, the same store [`store_operator_item`] writes:
/// a machine whose store is a file reads the session it stored there instead
/// of asking a Skarbiec it does not have. The whole-item form is refused by a
/// broker that requires a named field, and every caller here knows the field
/// it wants.
pub(super) async fn credential_field(role: &str, field: &str) -> Result<Option<String>, CmdError> {
    crate::credential_store::read_string(role, field)
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
    let mut value = json!({
        "display_name": "Stado Azure operator session",
        "login_email": account,
        "tenant_id": tenant,
        "client_id": AZURE_CLI_CLIENT_ID,
        "refresh_token": refresh_token,
        "credential_status": "ready",
        "tags": ["wisent", "azure", "operator", "oauth-refresh"]
    });
    // `scope` and `client_info` are stored only when the token endpoint
    // returned them: a scope filled in from the one requested, or an empty
    // string, read back as a grant Azure never stated.
    if let Some(scope) = token_body.get("scope").and_then(Value::as_str) {
        value["scope"] = json!(scope);
    }
    if let Some(info) = token_body.get("client_info").and_then(Value::as_str) {
        value["client_info"] = json!(info);
    }
    // `stado-secret` is the canonical kind that carries arbitrary named fields.
    // `oauth-client` is not it: that kind allows only a client id and secret, so
    // a session with a refresh token, tenant and scope is refused by the schema.
    crate::credential_store::write::write_role_item_with(role, "stado-secret", &value, &json!({}))
        .await
        .map(|_written_item| ())
        .map_err(|error| {
            CmdError::click(format!("cannot store Azure operator credential: {error}"))
                .stating(error.failure_code())
        })
}
