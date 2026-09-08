//! The browser half of the authorization-code grant: the PKCE pair, the
//! authorize URL, the loopback listener that receives the redirect, and the
//! token endpoint that turns the code into a refresh credential.

use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;
use uuid::Uuid;

use super::super::{
    auth_timeout, callback_chunk_size, callback_limit, header_end_len, one, CmdError, ARM_SCOPE,
    AZURE_CLI_CLIENT_ID,
};

pub(super) fn pkce_pair() -> (String, String) {
    let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    (verifier, challenge)
}

pub(super) fn authorization_url(
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

pub(super) fn open_system_browser(url: &str) -> Result<(), CmdError> {
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

pub(super) async fn receive_authorization_code(
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

pub(super) async fn exchange_authorization_code(
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
