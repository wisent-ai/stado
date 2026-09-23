use serde::Deserialize;
use serde_json::{json, Value};

use super::{provider_client, HandlerError, HandlerResult};

const TWILIO_ITEM: &str = "most-twilio";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendSmsRequest {
    to: String,
    text: String,
}

pub(super) fn supports(action: &str) -> bool {
    action == "send-sms"
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    match action {
        "send-sms" => send_sms(body).await,
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

fn phone_number(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return None;
    }
    Some(trimmed.to_string())
}

async fn send_sms(body: &[u8]) -> HandlerResult {
    let request: SendSmsRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    let to = phone_number(&request.to).ok_or(HandlerError::BadRequest)?;
    let text = request.text.trim();
    if text.is_empty() || text != request.text {
        return Err(HandlerError::BadRequest);
    }

    let provider = provider_client("most").await?;
    let account_sid = provider.read_string(TWILIO_ITEM, "account_sid").await?;
    let auth_token = provider.read_string(TWILIO_ITEM, "auth_token").await?;
    let api_version = provider.read_string(TWILIO_ITEM, "api_version").await?;
    let item = provider.read_item(TWILIO_ITEM).await?;
    if !valid_path_component(&account_sid) || !valid_path_component(&api_version) {
        return Err(HandlerError::ProviderUnavailable);
    }
    let messaging_service_sid = item
        .get("messaging_service_sid")
        .and_then(Value::as_str)
        .filter(|value| valid_path_component(value));
    let from_number = item
        .get("from_number")
        .and_then(Value::as_str)
        .and_then(phone_number);
    if messaging_service_sid.is_none() && from_number.is_none() {
        return Err(HandlerError::ProviderUnavailable);
    }

    let endpoint =
        format!("https://api.twilio.com/{api_version}/Accounts/{account_sid}/Messages.json");
    let mut form = vec![("To", to), ("Body", request.text)];
    if let Some(service_sid) = messaging_service_sid {
        form.push(("MessagingServiceSid", service_sid.to_string()));
    } else if let Some(from) = from_number {
        form.push(("From", from));
    }
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| HandlerError::ProviderUnavailable)?
        .post(endpoint)
        .basic_auth(&account_sid, Some(&auth_token))
        .form(&form)
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
