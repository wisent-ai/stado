//! Durable Azure operator authentication and RBAC repair.
//!
//! `azure login` uses authorization-code + PKCE against the target tenant.
//! `domain_hint=live.com` preserves the Microsoft-account federation used by
//! Azure refresh credential is written to the globally selected credential
//! store; authorization codes and access tokens remain process-local.

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;
use uuid::Uuid;

use super::CmdError;

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

fn pkce_pair() -> (String, String) {
    let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    (verifier, challenge)
}

fn authorization_url(
    tenant: &str,
    account: &str,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
) -> Result<Url, CmdError> {
    let mut url = Url::parse(&format!(
        "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"
    ))
    .map_err(|err| CmdError::click(err.to_string()))?;
    url.query_pairs_mut()
        .append_pair("client_id", AZURE_CLI_CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_mode", "query")
        .append_pair("scope", ARM_SCOPE)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("login_hint", account)
        .append_pair("domain_hint", "live.com");
    Ok(url)
}

fn open_system_browser(url: &str) -> Result<(), CmdError> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("/usr/bin/open");
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    command
        .arg(url)
        .spawn()
        .map_err(|err| CmdError::click(format!("cannot open Azure login URL: {err}")))?;
    Ok(())
}

async fn receive_authorization_code(
    listener: TcpListener,
    expected_state: &str,
) -> Result<String, CmdError> {
    let (mut stream, _) = tokio::time::timeout(auth_timeout(), listener.accept())
        .await
        .map_err(|_| CmdError::click("Azure login timed out waiting for the browser callback"))??;
    let mut request = Vec::new();
    let mut chunk = vec![u8::default(); callback_chunk_size()];
    while request.len() < callback_limit() {
        let count = stream.read(&mut chunk).await?;
        if count == usize::default() {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
        if request
            .windows(header_end_len())
            .any(|window| window == b"\r\n\r\n")
        {
            break;
        }
    }
    let first_line = String::from_utf8_lossy(&request)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let target = first_line
        .split_whitespace()
        .nth(one())
        .ok_or_else(|| CmdError::click("invalid Azure OAuth callback"))?;
    let callback = Url::parse(&format!("http://localhost{target}"))
        .map_err(|err| CmdError::click(format!("invalid Azure OAuth callback: {err}")))?;
    let params: std::collections::HashMap<_, _> = callback.query_pairs().into_owned().collect();
    let state_matches = params.get("state").map(String::as_str) == Some(expected_state);
    let result = if !state_matches {
        Err(CmdError::click("Azure OAuth callback state mismatch"))
    } else if let Some(error) = params.get("error") {
        Err(CmdError::click(format!(
            "Azure login failed: {error}: {}",
            params
                .get("error_description")
                .map(String::as_str)
                .unwrap_or("")
        )))
    } else {
        params
            .get("code")
            .filter(|code| !code.is_empty())
            .cloned()
            .ok_or_else(|| CmdError::click("Azure OAuth callback has no authorization code"))
    };
    let (status, message) = if result.is_ok() {
        (
            "200 OK",
            "Azure login completed. You can close this window.",
        )
    } else {
        (
            "400 Bad Request",
            "Azure login failed. Return to the terminal for details.",
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{message}",
        message.len()
    );
    stream.write_all(response.as_bytes()).await?;
    result
}

async fn exchange_authorization_code(
    tenant: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<Value, CmdError> {
    let response = reqwest::Client::new()
        .post(format!(
            "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"
        ))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", AZURE_CLI_CLIENT_ID),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
            ("scope", ARM_SCOPE),
        ])
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let body: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"detail": text}));
    if !status.is_success() {
        return Err(CmdError::click(format!(
            "Azure authorization-code exchange failed with HTTP {status}: {}",
            body.get("error_description")
                .or_else(|| body.get("detail"))
                .and_then(Value::as_str)
                .unwrap_or("unknown OAuth error")
        )));
    }
    Ok(body)
}

fn home_path(relative: &str) -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(relative)
}

fn credential_client() -> Result<crate::skarbiec::Client, CmdError> {
    let credentials = crate::credential_store::admin_credentials()
        .map_err(|error| CmdError::click(error.to_string()))?;
    crate::skarbiec::Client::new(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
    )
    .map_err(|error| CmdError::click(error.to_string()))
}

async fn credential_item(id: &str) -> Result<Value, CmdError> {
    credential_client()?
        .read_item(id)
        .await
        .map_err(|error| CmdError::click(format!("cannot read credential item {id}: {error}")))
}

async fn store_operator_item(
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
    credential_client()?
        .write_item(id, "oauth", &value)
        .await
        .map_err(|error| {
            CmdError::click(format!("cannot store Azure operator credential: {error}"))
        })
}

async fn login(args: LoginArgs) -> Result<(), CmdError> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let redirect_uri = format!("http://localhost:{}", listener.local_addr()?.port());
    let state = Uuid::new_v4().simple().to_string();
    let (verifier, challenge) = pkce_pair();
    let url = authorization_url(
        &args.tenant,
        &args.account,
        &redirect_uri,
        &state,
        &challenge,
    )?;
    println!("Azure login URL:\n{url}");
    if !args.no_open {
        open_system_browser(url.as_str())?;
    }
    let code = receive_authorization_code(listener, &state).await?;
    let body = exchange_authorization_code(&args.tenant, &code, &redirect_uri, &verifier).await?;
    let refresh_token = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click("Azure token response has no refresh_token"))?;
    store_operator_item(
        &args.item,
        &args.tenant,
        &args.account,
        refresh_token,
        &body,
    )
    .await?;
    let claims = jwt_claims(
        body.get("access_token")
            .and_then(Value::as_str)
            .unwrap_or(""),
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "authenticated": true,
            "account": claims.get("preferred_username").and_then(Value::as_str).unwrap_or(&args.account),
            "tenant_id": claims.get("tid").and_then(Value::as_str).unwrap_or(&args.tenant),
            "object_id": claims.get("oid").and_then(Value::as_str),
            "credential": args.item,
            "stored": "Skarbiec"
        }))?
    );
    Ok(())
}

fn jwt_claims(token: &str) -> Value {
    let Some(payload) = token.split('.').nth(one()) else {
        return Value::Null;
    };
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload));
    decoded
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

struct OperatorToken {
    access_token: String,
    tenant_id: String,
    account: String,
}

async fn refresh_operator_token(item_id: &str) -> Result<OperatorToken, CmdError> {
    let item = credential_item(item_id).await?;
    let required = |name: &str| {
        item.get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CmdError::click(format!("Skarbiec item {item_id} field {name} is required"))
            })
    };
    let tenant_id = required("tenant_id")?.to_string();
    let client_id = required("client_id")?.to_string();
    let refresh_token = required("refresh_token")?.to_string();
    let account = item
        .get("login_email")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let response = reqwest::Client::new()
        .post(format!(
            "https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token"
        ))
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id.as_str()),
            ("refresh_token", refresh_token.as_str()),
            ("scope", ARM_SCOPE),
        ])
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let body: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"detail": text}));
    if !status.is_success() {
        return Err(CmdError::click(format!(
            "Azure refresh-token exchange failed with HTTP {status}: {}",
            body.get("error_description")
                .or_else(|| body.get("detail"))
                .and_then(Value::as_str)
                .unwrap_or("unknown OAuth error")
        )));
    }
    let access_token = body
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click("Azure refresh response has no access_token"))?
        .to_string();
    if let Some(rotated) = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|value| *value != refresh_token)
    {
        store_operator_item(item_id, &tenant_id, &account, rotated, &body).await?;
    }
    Ok(OperatorToken {
        access_token,
        tenant_id,
        account,
    })
}

fn role_assignment_name(scope: &str, principal_id: &str, role_id: &str) -> Uuid {
    let material = format!("{scope}\n{principal_id}\n{role_id}");
    let digest = Sha256::digest(material.as_bytes());
    Uuid::from_slice(&digest[..parsed("16")]).expect("SHA digest prefix is a UUID")
}

async fn ensure_role(
    http: &reqwest::Client,
    access_token: &str,
    subscription: &str,
    scope: &str,
    principal_id: &str,
    role_name: &str,
    role_id: &str,
) -> Result<Value, CmdError> {
    let assignment = role_assignment_name(scope, principal_id, role_id);
    let response = http
        .put(format!(
            "{ARM_RESOURCE}{scope}/providers/Microsoft.Authorization/roleAssignments/{assignment}?api-version={ROLE_API_VERSION}"
        ))
        .bearer_auth(access_token)
        .json(&json!({
            "properties": {
                "roleDefinitionId": format!(
                    "/subscriptions/{subscription}/providers/Microsoft.Authorization/roleDefinitions/{role_id}"
                ),
                "principalId": principal_id,
                "principalType": "ServicePrincipal"
            }
        }))
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let error_code = body
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or("");
    let ok = status.is_success() || error_code == "RoleAssignmentExists";
    Ok(json!({
        "role": role_name,
        "scope": scope,
        "principal_id": principal_id,
        "ok": ok,
        "http_status": status.as_u16(),
        "outcome": if status.is_success() { "applied" } else if ok { "already_present" } else { "failed" },
        "error": if ok { Value::Null } else { json!({
            "code": error_code,
            "message": body.pointer("/error/message").and_then(Value::as_str).unwrap_or("Azure role assignment failed")
        }) }
    }))
}

async fn control_principal_id(args: &RepairRbacArgs) -> Result<String, CmdError> {
    if let Some(id) = args
        .principal_object_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return Ok(id.to_string());
    }
    let http = reqwest::Client::new();
    let token = crate::azure_token::identity_bearer_token(&http, ARM_SCOPE, ARM_RESOURCE)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    jwt_claims(&token)
        .get("oid")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CmdError::click("stado-azure ARM token has no oid claim"))
}

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

async fn discover_storage_account(
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

async fn agent_principal_id(
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

async fn handle_deny_assignments(
    http: &reqwest::Client,
    access_token: &str,
    subscription_scope: &str,
    remove_name: Option<&str>,
) -> Result<Value, CmdError> {
    let response = http
        .get(format!(
            "{ARM_RESOURCE}{subscription_scope}/providers/Microsoft.Authorization/denyAssignments?api-version={ROLE_API_VERSION}"
        ))
        .bearer_auth(access_token)
        .send()
        .await?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Ok(json!({
            "readable": false,
            "http_status": status.as_u16(),
            "error": body.get("error")
        }));
    }
    let mut reports = Vec::new();
    for assignment in body
        .get("value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = assignment.get("name").and_then(Value::as_str).unwrap_or("");
        let display_name = assignment
            .pointer("/properties/denyAssignmentName")
            .or_else(|| assignment.pointer("/properties/name"))
            .and_then(Value::as_str)
            .unwrap_or(name);
        let system_protected = assignment
            .pointer("/properties/isSystemProtected")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let selected = remove_name
            .map(|needle| display_name.contains(needle) || name.contains(needle))
            .unwrap_or(false);
        let mut report = json!({
            "name": name,
            "display_name": display_name,
            "system_protected": system_protected,
            "selected": selected,
            "outcome": "reported"
        });
        if selected && system_protected {
            report["outcome"] = Value::String("microsoft_support_required".into());
        } else if selected {
            let scope = assignment
                .pointer("/properties/scope")
                .and_then(Value::as_str)
                .unwrap_or(subscription_scope);
            let delete = http
                .delete(format!(
                    "{ARM_RESOURCE}{scope}/providers/Microsoft.Authorization/denyAssignments/{name}?api-version={ROLE_API_VERSION}"
                ))
                .bearer_auth(access_token)
                .send()
                .await?;
            report["http_status"] = Value::from(delete.status().as_u16());
            report["outcome"] = Value::String(
                if delete.status().is_success() {
                    "removed"
                } else {
                    "remove_failed"
                }
                .into(),
            );
        }
        reports.push(report);
    }
    Ok(json!({"readable": true, "assignments": reports}))
}

fn configured_subscription(explicit: Option<&str>) -> Result<String, CmdError> {
    let subscription = explicit
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::config::azure_subscription_id().to_string());
    if subscription.is_empty() {
        Err(CmdError::usage(
            "--subscription or AZURE_SUBSCRIPTION_ID is required",
        ))
    } else {
        Ok(subscription)
    }
}

async fn azure_get_json(
    http: &reqwest::Client,
    access_token: &str,
    url: &str,
) -> Result<Value, CmdError> {
    let response = http.get(url).bearer_auth(access_token).send().await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let body: Value =
        serde_json::from_str(&text).unwrap_or_else(|_| json!({"detail": text.trim()}));
    if status.is_success() {
        Ok(body)
    } else {
        Err(CmdError::click(format!(
            "Azure request failed with HTTP {status}: {}",
            body.pointer("/error/message")
                .or_else(|| body.get("detail"))
                .and_then(Value::as_str)
                .unwrap_or("unknown ARM error")
        )))
    }
}

async fn azure_collection(
    http: &reqwest::Client,
    access_token: &str,
    first_url: String,
) -> Result<Vec<Value>, CmdError> {
    let mut url = Some(first_url);
    let mut rows = Vec::new();
    while let Some(page_url) = url.take() {
        let page = azure_get_json(http, access_token, &page_url).await?;
        rows.extend(
            page.get("value")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        url = page
            .get("nextLink")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    Ok(rows)
}

fn deny_display_name(assignment: &Value) -> &str {
    assignment
        .pointer("/properties/denyAssignmentName")
        .or_else(|| assignment.pointer("/properties/name"))
        .and_then(Value::as_str)
        .or_else(|| assignment.get("name").and_then(Value::as_str))
        .unwrap_or("")
}

async fn list_unusual_activity_denies(
    http: &reqwest::Client,
    access_token: &str,
    subscription: &str,
) -> Result<Vec<Value>, CmdError> {
    let assignments = azure_collection(
        http,
        access_token,
        format!(
            "{ARM_RESOURCE}/subscriptions/{subscription}/providers/Microsoft.Authorization/denyAssignments?api-version={ROLE_API_VERSION}"
        ),
    )
    .await?;
    Ok(assignments
        .into_iter()
        .filter(|assignment| {
            assignment
                .pointer("/properties/isSystemProtected")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && deny_display_name(assignment)
                    .to_ascii_lowercase()
                    .contains("unusualactivity")
        })
        .map(|assignment| {
            json!({
                "id": assignment.get("id"),
                "name": assignment.get("name"),
                "display_name": deny_display_name(&assignment),
                "scope": assignment.pointer("/properties/scope"),
                "system_protected": assignment.pointer("/properties/isSystemProtected"),
                "applies_to_children": assignment
                    .pointer("/properties/doNotApplyToChildScopes")
                    .and_then(Value::as_bool)
                    .map(|value| !value),
                "principals": assignment.pointer("/properties/principals"),
                "excluded_principals": assignment.pointer("/properties/excludePrincipals"),
                "permissions": assignment.pointer("/properties/permissions"),
                "resolution": "microsoft_support_required"
            })
        })
        .collect())
}

fn support_display_name(row: &Value) -> &str {
    row.pointer("/properties/displayName")
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn find_support_row<'a>(rows: &'a [Value], alternatives: &[&[&str]]) -> Option<&'a Value> {
    alternatives.iter().find_map(|needles| {
        rows.iter().find(|row| {
            let display = support_display_name(row).to_ascii_lowercase();
            needles
                .iter()
                .all(|needle| display.contains(&needle.to_ascii_lowercase()))
        })
    })
}

async fn discover_rbac_support_classification(
    http: &reqwest::Client,
    access_token: &str,
) -> Result<(Value, Value), CmdError> {
    let services = azure_collection(
        http,
        access_token,
        format!(
            "{ARM_RESOURCE}/providers/Microsoft.Support/services?api-version={SUPPORT_API_VERSION}"
        ),
    )
    .await?;
    let service = find_support_row(
        &services,
        &[
            &["role based access control", "azure resources"],
            &["role based access control"],
            &["subscription management"],
        ],
    )
    .cloned()
    .ok_or_else(|| {
        CmdError::click("Azure Support did not return an RBAC or subscription-management service")
    })?;
    let service_name = service
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click("Azure Support RBAC service has no name"))?;
    let classifications = azure_collection(
        http,
        access_token,
        format!(
            "{ARM_RESOURCE}/providers/Microsoft.Support/services/{service_name}/problemClassifications?api-version={SUPPORT_API_VERSION}"
        ),
    )
    .await?;
    let classification = find_support_row(
        &classifications,
        &[
            &["problem", "rbac", "role assignment"],
            &["rbac", "role assignment"],
            &["role assignment"],
            &["permissions"],
        ],
    )
    .cloned()
    .ok_or_else(|| {
        let names = classifications
            .iter()
            .map(support_display_name)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        CmdError::click(format!(
            "Azure Support returned no RBAC role-assignment classification; available: {names}"
        ))
    })?;
    Ok((service, classification))
}

fn deny_ticket_description(tenant: &str, subscription: &str, denies: &[Value]) -> String {
    let assignments = denies
        .iter()
        .map(|deny| {
            format!(
                "- ID: {}; name: {}; scope: {}; principals: {}",
                deny.get("id").and_then(Value::as_str).unwrap_or("unknown"),
                deny.get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                deny.get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                deny.get("principals")
                    .map(Value::to_string)
                    .unwrap_or_else(|| "[]".into())
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Microsoft Azure created system-protected [UnusualActivity] Full Deny assignments at root scope. They are inherited by the subscription and deny administrative Actions and DataActions despite existing RBAC grants.\n\nTenant ID: {tenant}\nSubscription ID: {subscription}\nAssignments:\n{assignments}\n\nThe assignments are read-only and system-protected in Azure Portal. Please investigate the unusual-activity signal and remove all listed assignments after security verification."
    )
}

async fn find_existing_unusual_activity_ticket(
    http: &reqwest::Client,
    access_token: &str,
    subscription: &str,
) -> Result<Option<Value>, CmdError> {
    let tickets = azure_collection(
        http,
        access_token,
        format!(
            "{ARM_RESOURCE}/subscriptions/{subscription}/providers/Microsoft.Support/supportTickets?api-version={SUPPORT_API_VERSION}"
        ),
    )
    .await?;
    Ok(tickets.into_iter().find(|ticket| {
        ticket
            .pointer("/properties/status")
            .and_then(Value::as_str)
            .is_some_and(|status| status.eq_ignore_ascii_case("open"))
            && ticket.pointer("/properties/title").and_then(Value::as_str)
                == Some(UNUSUAL_ACTIVITY_TITLE)
    }))
}

async fn create_unusual_activity_ticket(
    http: &reqwest::Client,
    operator: &OperatorToken,
    args: &OpenUnusualActivityTicketArgs,
    subscription: &str,
    denies: &[Value],
) -> Result<Value, CmdError> {
    if let Ok(Some(existing)) =
        find_existing_unusual_activity_ticket(http, &operator.access_token, subscription).await
    {
        return Ok(json!({
            "outcome": "already_open",
            "ticket": existing
        }));
    }
    // UnusualActivity denies all Support reads but explicitly exempt Support
    // writes. Prefer current catalog IDs; fall back to the IDs returned by the
    // Azure portal when the deny prevents dynamic discovery.
    let (service, classification) = discover_rbac_support_classification(
        http,
        &operator.access_token,
    )
    .await
    .unwrap_or_else(|_| {
        (
            json!({
                "id": RBAC_SUPPORT_SERVICE_ID,
                "properties": {
                    "displayName": "Role Based Access Control (RBAC) for Azure Resources (IAM)"
                }
            }),
            json!({
                "id": RBAC_SUPPORT_CLASSIFICATION_ID,
                "properties": {
                    "displayName": "Problem with RBAC role assignments"
                }
            }),
        )
    });
    let service_id = service
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("Azure Support RBAC service has no id"))?;
    let classification_id = classification
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("Azure Support RBAC classification has no id"))?;
    let email = args
        .email
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(&operator.account);
    if email.is_empty() || args.first_name.is_empty() || args.last_name.is_empty() {
        return Err(CmdError::usage(
            "--first-name, --last-name, and a contact email are required",
        ));
    }
    let mut deny_ids = denies
        .iter()
        .filter_map(|deny| deny.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    deny_ids.sort_unstable();
    let digest = Sha256::digest(deny_ids.join("\n").as_bytes());
    let ticket_uuid =
        Uuid::from_slice(&digest[..parsed("16")]).expect("SHA digest prefix is a UUID");
    let ticket_name = format!("stado-unusual-activity-{}", ticket_uuid.simple());
    let ticket_url = format!(
        "{ARM_RESOURCE}/subscriptions/{subscription}/providers/Microsoft.Support/supportTickets/{ticket_name}?api-version={SUPPORT_API_VERSION}"
    );
    let response = http
        .put(&ticket_url)
        .bearer_auth(&operator.access_token)
        .json(&json!({
            "properties": {
                "title": UNUSUAL_ACTIVITY_TITLE,
                "description": deny_ticket_description(
                    &operator.tenant_id,
                    subscription,
                    denies
                ),
                "advancedDiagnosticConsent": "No",
                "contactDetails": {
                    "country": args.country,
                    "firstName": args.first_name,
                    "lastName": args.last_name,
                    "preferredContactMethod": "email",
                    "preferredSupportLanguage": "en-US",
                    "preferredTimeZone": args.time_zone,
                    "primaryEmailAddress": email
                },
                "problemClassificationId": classification_id,
                "serviceId": service_id,
                "supportPlanId": STANDARD_SUPPORT_PLAN_ID,
                "severity": "minimal",
                "require24X7Response": false,
            }
        }))
        .send()
        .await?;
    let status = response.status();
    let operation_url = response
        .headers()
        .get("location")
        .or_else(|| response.headers().get("azure-asyncoperation"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let text = response.text().await.unwrap_or_default();
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(CmdError::click(format!(
            "Azure Support ticket creation failed with HTTP {status}: {}",
            body.get("error").unwrap_or(&body)
        )));
    }
    if status.as_u16() == parsed::<u16>("202") {
        if let Some(url) = operation_url {
            for _ in usize::default()..parsed::<usize>("30") {
                tokio::time::sleep(Duration::from_secs(parsed("2"))).await;
                let poll = http
                    .get(&url)
                    .bearer_auth(&operator.access_token)
                    .send()
                    .await?;
                if poll.status().as_u16() != parsed::<u16>("202") {
                    break;
                }
            }
        }
    }
    let ticket = azure_get_json(http, &operator.access_token, &ticket_url)
        .await
        .unwrap_or_else(|_| {
            json!({
                "id": format!("/subscriptions/{subscription}/providers/Microsoft.Support/supportTickets/{ticket_name}"),
                "name": ticket_name.clone(),
                "properties": {
                    "status": "Submitted; status read is blocked by the active deny assignment",
                    "title": UNUSUAL_ACTIVITY_TITLE
                }
            })
        });
    Ok(json!({
        "outcome": "created",
        "ticket_name": ticket_name,
        "service": support_display_name(&service),
        "problem_classification": support_display_name(&classification),
        "ticket": ticket
    }))
}

fn persist_unusual_activity_receipt(receipt: &Value) -> Result<PathBuf, CmdError> {
    let path = home_path(".stado/azure-unusual-activity-ticket.json");
    let temporary = path.with_extension("json.tmp");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut content = serde_json::to_vec_pretty(receipt)?;
    content.push(b'\n');
    std::fs::write(&temporary, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let radix = parsed("8");
        let mode = u32::from_str_radix("600", radix).expect("owner-only mode is valid octal");
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(mode))?;
    }
    std::fs::rename(&temporary, &path)?;
    Ok(path)
}

async fn unusual_activity(args: UnusualActivityArgs) -> Result<(), CmdError> {
    match args.command {
        UnusualActivityCommands::Diagnose(args) => {
            let subscription = configured_subscription(args.subscription.as_deref())?;
            let operator = refresh_operator_token(&args.operator_item).await?;
            let http = reqwest::Client::new();
            let denies =
                list_unusual_activity_denies(&http, &operator.access_token, &subscription).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "operator": {
                        "account": operator.account,
                        "tenant_id": operator.tenant_id,
                        "credential": args.operator_item
                    },
                    "subscription_id": subscription,
                    "active_unusual_activity_denies": denies.len(),
                    "deny_assignments": denies,
                    "resolution": if denies.is_empty() {
                        "no_active_unusual_activity_deny"
                    } else {
                        "run `stado azure unusual-activity open-ticket ... --confirm`; Azure Support must remove system-protected assignments"
                    }
                }))?
            );
            Ok(())
        }
        UnusualActivityCommands::OpenTicket(args) => {
            if !args.confirm {
                return Err(CmdError::usage(
                    "--confirm is required because this command creates an external Azure Support case",
                ));
            }
            let subscription = configured_subscription(args.common.subscription.as_deref())?;
            let operator = refresh_operator_token(&args.common.operator_item).await?;
            let http = reqwest::Client::new();
            let denies =
                list_unusual_activity_denies(&http, &operator.access_token, &subscription).await?;
            if denies.is_empty() {
                return Err(CmdError::click(
                    "no active system-protected UnusualActivity deny assignment was found",
                ));
            }
            let result =
                create_unusual_activity_ticket(&http, &operator, &args, &subscription, &denies)
                    .await?;
            let mut receipt = json!({
                "saved_at": chrono::Utc::now().to_rfc3339(),
                "subscription_id": subscription,
                "deny_assignment_count": denies.len(),
                "support_request": result
            });
            let receipt_path = home_path(".stado/azure-unusual-activity-ticket.json");
            receipt["receipt_file"] = Value::String(receipt_path.to_string_lossy().into_owned());
            persist_unusual_activity_receipt(&receipt)?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
            Ok(())
        }
    }
}

async fn repair_rbac(args: RepairRbacArgs) -> Result<(), CmdError> {
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
