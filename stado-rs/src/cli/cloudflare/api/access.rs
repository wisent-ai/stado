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
        )
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    let tunnel_id = required_field(tunnel_credential_name, "tunnel_id").await?;
    let api_token = required_field(api_credential_name, "api_token").await?;
    // These two ids come from the stored credentials, not from Cloudflare, so
    // a malformed one is the operator's configuration.
    validate_api_component("account_id", &account_id)
        .map_err(|error| error.stating(crate::primitives::failure::FailureCode::Config))?;
    validate_api_component("tunnel_id", &tunnel_id)
        .map_err(|error| error.stating(crate::primitives::failure::FailureCode::Config))?;
    Ok(TunnelAccess {
        account_id,
        tunnel_id,
        client: CloudflareClient::new(api_token)?,
    })
}

/// The account a zone is created in and the client that creates it, from one
/// credential holding the token in `api_key`, the field Weles writes an
/// acquired token to. The account is the one the token reaches: Cloudflare's
/// `/accounts` must answer exactly one, so a token that reaches several is
/// refused instead of creating the zone in whichever came first.
pub(in crate::cli::cloudflare) async fn account_access(
    api_credential_name: &str,
) -> Result<(String, CloudflareClient), CmdError> {
    crate::cli::host::settle_consumer_reads(api_credential_name, &["api_key"]).await?;
    let api_token = required_field(api_credential_name, "api_key").await?;
    let client = CloudflareClient::new(api_token)?;
    let accounts = client.get("/accounts", &[]).await?;
    let ids: Vec<String> = super::result_array(&accounts, "Cloudflare account list")?
        .iter()
        .filter_map(|account| account.get("id").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect();
    let [account_id] = ids.as_slice() else {
        return Err(CmdError::click(format!(
            "the token in {api_credential_name} reaches {} Cloudflare accounts; it must reach \
             exactly one",
            ids.len()
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    };
    validate_api_component("account_id", account_id)?;
    Ok((account_id.clone(), client))
}

/// One required credential field of the item the caller names, read as named:
/// a role lookup finds no item playing a role called by that name.
pub(in crate::cli::cloudflare) async fn required_field(
    item: &str,
    field: &str,
) -> Result<String, CmdError> {
    crate::credential_store::read_declared_string(item, field)
        .await
        .map_err(CmdError::from)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "credential field {field:?} of {item:?} is required"
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })
}
