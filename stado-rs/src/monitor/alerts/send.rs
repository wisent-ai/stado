//! Channel delivery functions, split from `alerts.rs` to keep both files
//! under the repository's per-file line budget. Resolution (Skarbiec reads)
//! stays in `alerts.rs::AlertChannels::from_env`; everything here is pure
//! HTTP against an already-resolved channel struct.

use serde_json::json;

use super::{MostChannel, PubSubChannel, SendgridChannel, TelegramChannel};

/// Error on a non-2xx response, including the upstream body.
async fn ensure_success(response: reqwest::Response) -> Result<(), String> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    Err(format!("HTTP {status}: {body}"))
}

pub(super) async fn send_slack(
    client: &reqwest::Client,
    url: &str,
    message: &str,
) -> Result<(), String> {
    let response = client
        .post(url)
        .json(&json!({"text": message}))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    ensure_success(response).await
}

pub(super) async fn send_telegram(
    client: &reqwest::Client,
    channel: &TelegramChannel,
    message: &str,
) -> Result<(), String> {
    let url = format!("{}/bot{}/sendMessage", channel.api_base, channel.token);
    let response = client
        .post(url)
        .json(&json!({"chat_id": channel.chat_id, "text": message, "parse_mode": "Markdown"}))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    ensure_success(response).await
}

pub(super) async fn send_email(
    client: &reqwest::Client,
    channel: &SendgridChannel,
    subject: &str,
    body: &str,
) -> Result<(), String> {
    let response = client
        .post(&channel.url)
        .bearer_auth(&channel.api_key)
        .json(&json!({
            "personalizations": [{"to": [{"email": channel.to}]}],
            "from": {"email": channel.from},
            "subject": subject,
            "content": [{"type": "text/plain", "value": body}],
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    ensure_success(response).await
}

pub(super) async fn send_pubsub(
    client: &reqwest::Client,
    channel: &PubSubChannel,
    message: &str,
) -> Result<(), String> {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(message);
    let url = format!("{}/v1/{}:publish", channel.base_url, channel.topic);
    let response = client
        .post(url)
        .bearer_auth(&channel.token)
        .json(&json!({"messages": [{"data": data}]}))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    ensure_success(response).await
}

/// SMS through the `most` integration domain: the same Twilio Programmable
/// Messaging call the dashboard's `most/send-sms` action makes, but delivered
/// in-process — an alert path that routed through the local dashboard would
/// go quiet in exactly the outage it exists to report.
pub(super) async fn send_most(
    client: &reqwest::Client,
    channel: &MostChannel,
    message: &str,
) -> Result<(), String> {
    let endpoint = format!(
        "{}/{}/Accounts/{}/Messages.json",
        channel.api_base, channel.api_version, channel.account_sid
    );
    let mut form = vec![("To", channel.phone.clone()), ("Body", message.to_string())];
    if let Some(service_sid) = &channel.messaging_service_sid {
        form.push(("MessagingServiceSid", service_sid.clone()));
    } else if let Some(from) = &channel.from_number {
        form.push(("From", from.clone()));
    } else {
        return Err("most channel has neither messaging_service_sid nor from_number".to_string());
    }
    let response = client
        .post(endpoint)
        .basic_auth(&channel.account_sid, Some(&channel.auth_token))
        .form(&form)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    ensure_success(response).await
}

/// Python `subject or message[:80]`, char-boundary safe.
pub(super) fn email_subject<'a>(subject: &'a str, message: &'a str) -> String {
    if subject.is_empty() {
        message.chars().take(80).collect()
    } else {
        subject.to_string()
    }
}
