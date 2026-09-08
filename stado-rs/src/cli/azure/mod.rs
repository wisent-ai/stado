//! Durable Azure operator authentication and RBAC repair.
//!
//! `azure login` uses authorization-code + PKCE against the target tenant.
//! `domain_hint=live.com` preserves the Microsoft-account federation used by
//! Azure refresh credential is written to the globally selected credential
//! store; authorization codes and access tokens remain process-local.

use std::time::Duration;

use clap::{Args, Subcommand};

use super::CmdError;

mod diagnostics;
mod resources;
mod session;

use diagnostics::unusual_activity;
use resources::repair_rbac;
use session::login;

const AZURE_CLI_CLIENT_ID: &str = "04b07795-8ddb-461a-bbee-02f9e1bf7b46";
const DEFAULT_OPERATOR_ITEM: &str = "stado-azure-operator";
const ARM_SCOPE: &str = "https://management.azure.com/.default offline_access openid profile";
const ARM_RESOURCE: &str = "https://management.azure.com";
const ROLE_API_VERSION: &str = "2022-04-01";
const IDENTITY_API_VERSION: &str = "2023-01-31";
const STORAGE_API_VERSION: &str = "2023-05-01";
const SUPPORT_API_VERSION: &str = "2024-04-01";
const UNUSUAL_ACTIVITY_TITLE: &str =
    "System-protected UnusualActivity deny assignments block Azure administration";
const RBAC_SUPPORT_SERVICE_ID: &str =
    "/providers/Microsoft.Support/services/c2804d27-8e0a-f2a3-8540-f4318f539ff6";
const RBAC_SUPPORT_CLASSIFICATION_ID: &str = "/providers/Microsoft.Support/services/c2804d27-8e0a-f2a3-8540-f4318f539ff6/problemClassifications/149f350b-ec67-1d49-ea9f-b0bcde639e4d";
const STANDARD_SUPPORT_PLAN_ID: &str = "U291cmNlOkF6dXJlTW9kZXJuLFN1YnNjcmlwdGlvbklkOjlhZTdjZmE0LTkzZTQtNDRmNi04ZjRkLTVjZWE2NzBlMjJiZCxTb3ZlcmVpZ25DbG91ZDpQdWJsaWMsT2ZmZXJJZDpzdGFuZGFyZF9zdXBwb3J0LA==";

const CONTRIBUTOR_ROLE: &str = "b24988ac-6180-42a0-ab88-20f7382dd24c";
const STORAGE_BLOB_DATA_CONTRIBUTOR_ROLE: &str = "ba92f5b4-2d11-453d-a403-e96b0029c9fe";
const VIRTUAL_MACHINE_CONTRIBUTOR_ROLE: &str = "9980e02c-c2be-4d73-94e8-173b1dc7cf3c";
const QUOTA_REQUEST_OPERATOR_ROLE: &str = "0e5f05e5-9ab9-446b-b98d-1e2157c94125";
const SUPPORT_REQUEST_CONTRIBUTOR_ROLE: &str = "cfd33db0-3dd1-45e3-aa9d-cdbdf3b6f24e";

fn parsed<T: std::str::FromStr>(text: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    text.parse().expect("valid built-in number")
}

fn auth_timeout() -> Duration {
    Duration::from_secs(parsed("600"))
}

fn callback_limit() -> usize {
    parsed("16384")
}

fn callback_chunk_size() -> usize {
    parsed("2048")
}

fn header_end_len() -> usize {
    parsed("4")
}

fn one() -> usize {
    usize::from(true)
}

#[derive(Subcommand)]
pub enum AzureCommands {
    /// Sign in through Microsoft Account federation and encrypt the refresh token in Skarbiec.
    Login(LoginArgs),
    /// Apply Stado control-plane/agent roles and inspect a named deny assignment.
    #[command(name = "repair-rbac")]
    RepairRbac(RepairRbacArgs),
    /// Diagnose Azure's system-protected UnusualActivity deny and open an idempotent support case.
    #[command(name = "unusual-activity")]
    UnusualActivity(UnusualActivityArgs),
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
}

#[derive(Args)]
pub struct UnusualActivityArgs {
    #[command(subcommand)]
    command: UnusualActivityCommands,
}

#[derive(Subcommand)]
pub enum UnusualActivityCommands {
    /// Report inherited system-protected UnusualActivity deny assignments.
    Diagnose(UnusualActivityCommonArgs),
    /// Open one Azure Support case for the currently active assignments.
    #[command(name = "open-ticket")]
    OpenTicket(OpenUnusualActivityTicketArgs),
}

#[derive(Args)]
pub struct UnusualActivityCommonArgs {
    /// Azure subscription to inspect; defaults to AZURE_SUBSCRIPTION_ID/config.
    #[arg(long)]
    subscription: Option<String>,
    /// Owner-only Skarbiec item containing the operator refresh token.
    #[arg(long, default_value = DEFAULT_OPERATOR_ITEM)]
    operator_item: String,
}

#[derive(Args)]
pub struct OpenUnusualActivityTicketArgs {
    #[command(flatten)]
    common: UnusualActivityCommonArgs,
    /// Contact first name sent to Microsoft Support.
    #[arg(long)]
    first_name: String,
    /// Contact last name sent to Microsoft Support.
    #[arg(long)]
    last_name: String,
    /// Contact email; defaults to the Azure operator login.
    #[arg(long)]
    email: Option<String>,
    /// Contact country as an ISO 3166-1 alpha-3 code.
    #[arg(long, default_value = "POL")]
    country: String,
    /// Microsoft time-zone name used for support contact.
    #[arg(long, default_value = "Central European Standard Time")]
    time_zone: String,
    /// Required acknowledgement that this creates an external support case.
    #[arg(long)]
    confirm: bool,
}

pub async fn dispatch(command: AzureCommands) -> Result<(), CmdError> {
    match command {
        AzureCommands::Login(args) => login(args).await,
        AzureCommands::RepairRbac(args) => repair_rbac(args).await,
        AzureCommands::UnusualActivity(args) => unusual_activity(args).await,
    }
}

struct OperatorToken {
    access_token: String,
    tenant_id: String,
    account: String,
}
