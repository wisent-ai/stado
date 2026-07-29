use std::collections::BTreeMap;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use ring::hmac;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{provider_client, HandlerError, HandlerResult};

const TWILIO_ITEM: &str = "trading-tools-twilio";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendWhatsAppRequest {
    to: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyWhatsAppWebhookRequest {
    webhook_url: String,
    signature: String,
    form: BTreeMap<String, String>,
}

pub(super) fn supports(action: &str) -> bool {
    matches!(action, "send-whatsapp" | "verify-whatsapp-webhook")
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    match action {
        "send-whatsapp" => send_whatsapp(body).await,
        "verify-whatsapp-webhook" => verify_whatsapp_webhook(body).await,
        _ => Err(HandlerError::BadRequest),
    }
}

fn valid_path_component(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn whatsapp_number(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return None;
    }
    Some(if trimmed.starts_with("whatsapp:") {
        trimmed.to_string()
    } else {
        format!("whatsapp:{trimmed}")
    })
}

async fn twilio_credentials() -> Result<(String, String, String, String), HandlerError> {
    let provider = provider_client("trading").await?;
    let account_sid = provider.read_string(TWILIO_ITEM, "account_sid").await?;
    let auth_token = provider.read_string(TWILIO_ITEM, "auth_token").await?;
    let from_number = provider.read_string(TWILIO_ITEM, "whatsapp_from").await?;
    let api_version = provider.read_string(TWILIO_ITEM, "api_version").await?;
    if !valid_path_component(&account_sid) || !valid_path_component(&api_version) {
        return Err(HandlerError::ProviderUnavailable);
    }
    Ok((account_sid, auth_token, from_number, api_version))
}

async fn send_whatsapp(body: &[u8]) -> HandlerResult {
    let request: SendWhatsAppRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    let to = whatsapp_number(&request.to).ok_or(HandlerError::BadRequest)?;
    let text = request.text.trim();
    if text.is_empty() || text != request.text {
        return Err(HandlerError::BadRequest);
    }
    let (account_sid, auth_token, from_number, api_version) = twilio_credentials().await?;
    let from = whatsapp_number(&from_number).ok_or(HandlerError::ProviderUnavailable)?;
    let endpoint =
        format!("https://api.twilio.com/{api_version}/Accounts/{account_sid}/Messages.json");
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| HandlerError::ProviderUnavailable)?
        .post(endpoint)
        .basic_auth(&account_sid, Some(&auth_token))
        .form(&[("To", to), ("From", from), ("Body", request.text)])
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if !response.status().is_success() {
        return Err(HandlerError::UpstreamFailure);
    }
    let reply: Value = response
        .json()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let message_id = reply
        .get("sid")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(HandlerError::UpstreamFailure)?;
    Ok(json!({"message_id": message_id, "status": reply.get("status").and_then(Value::as_str)}))
}

async fn verify_whatsapp_webhook(body: &[u8]) -> HandlerResult {
    let request: VerifyWhatsAppWebhookRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    let parsed = reqwest::Url::parse(&request.webhook_url).map_err(|_| HandlerError::BadRequest)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(HandlerError::BadRequest);
    }
    let signature = match BASE64_STANDARD.decode(request.signature.as_bytes()) {
        Ok(value) => value,
        Err(_) => return Ok(json!({"valid": false})),
    };
    let provider = provider_client("trading").await?;
    let auth_token = provider.read_string(TWILIO_ITEM, "auth_token").await?;
    let mut signed = request.webhook_url;
    for (name, value) in request.form {
        signed.push_str(&name);
        signed.push_str(&value);
    }
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, auth_token.as_bytes());
    Ok(json!({"valid": hmac::verify(&key, signed.as_bytes(), &signature).is_ok()}))
}
