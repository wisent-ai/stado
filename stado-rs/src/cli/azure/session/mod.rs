//! The operator session: signing in once, and turning the stored refresh
//! token into a short-lived ARM access token for every later verb.

use base64::Engine;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use uuid::Uuid;

use super::{CmdError, LoginArgs, OperatorToken, ARM_SCOPE};

mod credentials;
mod grant;

use credentials::{credential_field, store_operator_item};
use grant::{
    authorization_url, exchange_authorization_code, open_system_browser, pkce_pair,
    receive_authorization_code,
};

pub(in crate::cli::azure) async fn login(args: LoginArgs) -> Result<(), CmdError> {
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
    // With --json stdout carries only the answer, so the URL a person opens
    // goes to stderr there.
    if args.json {
        eprintln!("Azure login URL:\n{url}");
    } else {
        println!("Azure login URL:\n{url}");
    }
    if !args.no_open {
        open_system_browser(url.as_str())?;
    }
    let code = receive_authorization_code(listener, &state).await?;
    let body = exchange_authorization_code(&args.tenant, &code, &redirect_uri, &verifier).await?;
    let refresh_token = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click("Azure token response has no refresh_token")
                .stating(crate::primitives::failure::FailureCode::Auth)
        })?;
    store_operator_item(
        &args.role,
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
    crate::cli::print_answer(
        &json!({
            "authenticated": true,
            "account": claims.get("preferred_username").and_then(Value::as_str).unwrap_or(&args.account),
            "tenant_id": claims.get("tid").and_then(Value::as_str).unwrap_or(&args.tenant),
            "object_id": claims.get("oid").and_then(Value::as_str),
            "credential_role": args.role,
            "stored": "Skarbiec"
        }),
        args.json,
    )
}

pub(in crate::cli::azure) fn jwt_claims(token: &str) -> Value {
    let Some(payload) = token
        .split_once('.')
        .and_then(|(_, rest)| rest.split('.').next())
    else {
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

pub(in crate::cli::azure) async fn refresh_operator_token(
    role: &str,
) -> Result<OperatorToken, CmdError> {
    let required = |value: Option<String>, name: &str| {
        value.filter(|value| !value.is_empty()).ok_or_else(|| {
            CmdError::click(format!(
                "the item playing role {role} has no {name}; run `stado azure login --role {role}`"
            ))
        })
    };
    let tenant_id = required(credential_field(role, "tenant_id").await?, "tenant_id")?;
    let client_id = required(credential_field(role, "client_id").await?, "client_id")?;
    let refresh_token = required(
        credential_field(role, "refresh_token").await?,
        "refresh_token",
    )?;
    let account = credential_field(role, "login_email")
        .await?
        .unwrap_or_default();
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
        ))
        .stating(crate::primitives::failure::FailureCode::from_upstream_status(status.as_u16())));
    }
    let access_token = body
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click("Azure refresh response has no access_token")
                .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?
        .to_string();
    // The operator grant is read-only by design. Microsoft may return a
    // replacement refresh token here, but persisting it would require a
    // distinct credential-lifecycle writer; never widen the reader grant.
    Ok(OperatorToken {
        access_token,
        tenant_id,
        account,
    })
}
