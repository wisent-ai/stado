//! The tunnel access scope: the account and tunnel every request is addressed
//! to, the client that carries it, and the credential fields both are read from.

use super::client::CloudflareClient;
use super::validate::validate_api_component;
use crate::cli::CmdError;

pub(in crate::cli::cloudflare) struct TunnelAccess {
    pub(in crate::cli::cloudflare) account_id: String,
    pub(in crate::cli::cloudflare) tunnel_id: String,
    pub(in crate::cli::cloudflare) client: CloudflareClient,
}

impl TunnelAccess {
    pub(in crate::cli::cloudflare) fn configuration_path(&self) -> String {
        format!(
            "/accounts/{}/cfd_tunnel/{}/configurations",
            self.account_id, self.tunnel_id
        )
    }

    pub(in crate::cli::cloudflare) fn connections_path(&self) -> String {
        format!(
            "/accounts/{}/cfd_tunnel/{}/connections",
            self.account_id, self.tunnel_id
        )
    }

    pub(in crate::cli::cloudflare) fn dns_content(&self) -> String {
        format!("{}.cfargotunnel.com", self.tunnel_id)
    }
}

pub(in crate::cli::cloudflare) async fn tunnel_access(
    api_credential_name: &str,
    tunnel_credential_name: &str,
) -> Result<TunnelAccess, CmdError> {
    // Named fields, not whole items: this broker refuses a read that names
    // none. Read-only lifecycle commands never acquire the connector token.
    let account_id = required_field(api_credential_name, "account_id").await?;
    let tunnel_account_id = required_field(tunnel_credential_name, "account_id").await?;
    if account_id != tunnel_account_id {
        return Err(CmdError::click(
            "Cloudflare API and tunnel credentials belong to different accounts",
        ));
    }
    let tunnel_id = required_field(tunnel_credential_name, "tunnel_id").await?;
    let api_token = required_field(api_credential_name, "api_token").await?;
    validate_api_component("account_id", &account_id)?;
    validate_api_component("tunnel_id", &tunnel_id)?;
    Ok(TunnelAccess {
        account_id,
        tunnel_id,
        client: CloudflareClient::new(api_token)?,
    })
}

/// One required credential field, read by name through the selected store.
pub(in crate::cli::cloudflare) async fn required_field(
    item: &str,
    field: &str,
) -> Result<String, CmdError> {
    crate::credential_store::read_string(item, field)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "credential field {field:?} of {item:?} is required"
            ))
        })
}
