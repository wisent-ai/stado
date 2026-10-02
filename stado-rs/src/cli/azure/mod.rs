//! Durable Azure operator authentication and RBAC repair.
//!
//! `azure login` uses authorization-code + PKCE against the target tenant.
//! `domain_hint=live.com` preserves the Microsoft-account federation used by
//! Azure refresh credential is written to the globally selected credential
//! store; authorization codes and access tokens remain process-local.

use clap::{Args, Subcommand};

use super::CmdError;

mod resources;
mod session;

use resources::repair_rbac;
use session::login;

const AZURE_CLI_CLIENT_ID: &str = "04b07795-8ddb-461a-bbee-02f9e1bf7b46";
const DEFAULT_OPERATOR_ITEM: &str = "stado-azure-operator";
const ARM_SCOPE: &str = "https://management.azure.com/.default offline_access openid profile";
const ARM_RESOURCE: &str = "https://management.azure.com";
const ROLE_API_VERSION: &str = "2022-04-01";
const IDENTITY_API_VERSION: &str = "2023-01-31";
const STORAGE_API_VERSION: &str = "2023-05-01";

const CONTRIBUTOR_ROLE: &str = "b24988ac-6180-42a0-ab88-20f7382dd24c";
const STORAGE_BLOB_DATA_CONTRIBUTOR_ROLE: &str = "ba92f5b4-2d11-453d-a403-e96b0029c9fe";
const VIRTUAL_MACHINE_CONTRIBUTOR_ROLE: &str = "9980e02c-c2be-4d73-94e8-173b1dc7cf3c";
const QUOTA_REQUEST_OPERATOR_ROLE: &str = "0e5f05e5-9ab9-446b-b98d-1e2157c94125";
const SUPPORT_REQUEST_CONTRIBUTOR_ROLE: &str = "cfd33db0-3dd1-45e3-aa9d-cdbdf3b6f24e";

#[derive(Subcommand)]
pub enum AzureCommands {
    /// Sign in through Microsoft Account federation and encrypt the refresh token in Skarbiec.
    Login(LoginArgs),
    /// Apply Stado control-plane/agent roles and inspect a named deny assignment.
    #[command(name = "repair-rbac")]
    RepairRbac(RepairRbacArgs),
}

#[derive(Args)]
pub struct LoginArgs {
    /// Azure tenant containing the guest account and subscription.
    #[arg(long)]
    tenant: String,
    /// Login hint for the federated Microsoft account.
    #[arg(long)]
    account: String,
    /// Owner-only Skarbiec item that receives the refresh token.
    #[arg(long, default_value = DEFAULT_OPERATOR_ITEM)]
    item: String,
    /// Print the authorization URL without launching the system browser.
    #[arg(long)]
    no_open: bool,
    /// Print the signed-in session as JSON instead of lines.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct RepairRbacArgs {
    /// Azure subscription to repair; defaults to AZURE_SUBSCRIPTION_ID/config.
    #[arg(long)]
    subscription: Option<String>,
    /// Resource group containing Stado compute resources.
    #[arg(long)]
    resource_group: Option<String>,
    /// Queue storage account; defaults to WC_AZURE_STORAGE_ACCOUNT/config.
    #[arg(long)]
    storage_account: Option<String>,
    /// Stado service-principal object id; otherwise decoded from its ARM token.
    #[arg(long)]
    principal_object_id: Option<String>,
    /// Agent managed-identity object id; otherwise resolved from AZURE_VM_IDENTITY_ID.
    #[arg(long)]
    agent_object_id: Option<String>,
    /// Owner-only Skarbiec item containing the operator refresh token.
    #[arg(long, default_value = DEFAULT_OPERATOR_ITEM)]
    operator_item: String,
    /// Exact substring of a deny-assignment display name to remove when Azure permits it.
    #[arg(long)]
    remove_deny_name: Option<String>,
    /// Print the repair report as JSON instead of lines.
    #[arg(long)]
    json: bool,
}

pub async fn dispatch(command: AzureCommands) -> Result<(), CmdError> {
    match command {
        AzureCommands::Login(args) => login(args).await,
        AzureCommands::RepairRbac(args) => repair_rbac(args).await,
    }
}

struct OperatorToken {
    access_token: String,
    tenant_id: String,
    account: String,
}
