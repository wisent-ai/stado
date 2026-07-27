//! Alert delivery: Slack webhook, Telegram Bot API, SendGrid mail, and GCP
//! Pub/Sub. Non-GCP channel credentials are resolved from the `stado-alerts`
//! Skarbiec item. Pub/Sub uses workload identity through `gcp_auth`.
//!
//! DEVIATION from Python (deliberate): Python lets a channel exception
//! propagate out of `send_alert`, suppressing every later channel. Here each
//! channel is fault-isolated — on error it logs `[alert] <channel> failed:
//! {err}` and the remaining channels still fire. Missing Skarbiec items or
//! non-secret routing config disable only their channel. A gcp_auth failure for
//! Pub/Sub likewise logs and skips.

use serde_json::{json, Value};

/// GCP OAuth scope for the Pub/Sub publish call.
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
/// Telegram Bot API base (overridable per-channel for tests).
const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
/// SendGrid mail-send endpoint.
const SENDGRID_URL: &str = "https://api.sendgrid.com/v3/mail/send";
/// Pub/Sub REST base.
const PUBSUB_BASE: &str = "https://pubsub.googleapis.com";
/// Python `WC_EMAIL_FROM` default.
const DEFAULT_EMAIL_FROM: &str = "compute@example.com";

fn log(msg: &str) {
    eprintln!("[alert] {msg}");
}

/// Telegram channel config (Skarbiec bot token + configured chat id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramChannel {
    pub token: String,
    pub chat_id: String,
    /// Bot API base URL; the request path is `/bot{token}/sendMessage`.
    pub api_base: String,
}

/// SendGrid channel config (`sendgrid_api_key` from Skarbiec plus non-secret
/// recipient/sender configuration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendgridChannel {
    pub api_key: String,
    pub to: String,
    pub from: String,
    pub url: String,
}

/// Pub/Sub channel config: full `projects/{p}/topics/{t}` topic path plus a
/// pre-fetched OAuth token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PubSubChannel {
    pub topic: String,
    pub base_url: String,
    pub token: String,
}

/// Resolved alert-channel configuration. Channels with no config are `None`
/// and are skipped. Construct via [`AlertChannels::from_env`] in production;
/// tests build literals pointing at the loopback mock (never env vars —
/// parallel tests would race on process env).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlertChannels {
    /// Slack webhook URL from `stado-alerts/slack_webhook` in Skarbiec.
    pub slack_webhook: Option<String>,
    pub telegram: Option<TelegramChannel>,
    pub sendgrid: Option<SendgridChannel>,
    pub pubsub: Option<PubSubChannel>,
}

impl AlertChannels {
    /// Resolve secret-bearing channel configuration from the `stado-alerts`
    /// Skarbiec item. Non-secret routing fields remain ordinary configuration.
    /// Pub/Sub uses workload identity through `gcp_auth`.
    pub async fn from_env(topic: &str) -> Self {
        let stored = match crate::skarbiec::Client::configured() {
            Ok(vault) => match vault.read_item("stado-alerts").await {
                Ok(value) => value,
                Err(err) => {
                    log(&format!("Skarbiec alert credentials unavailable: {err}"));
                    Value::Null
                }
            },
            Err(err) => {
                log(&format!("Skarbiec alert credentials unavailable: {err}"));
                Value::Null
            }
        };
        let secret = |field: &str| {
            stored
                .get(field)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let slack_webhook = secret("slack_webhook");

        let telegram = match (secret("telegram_bot_token"), secret("telegram_chat_id")) {
            (Some(token), Some(chat_id)) if !chat_id.is_empty() => Some(TelegramChannel {
                token,
                chat_id,
                api_base: TELEGRAM_API_BASE.to_string(),
            }),
            _ => None,
        };

        let sendgrid = match (
            secret("sendgrid_api_key"),
            std::env::var("WC_EMAIL_TO").ok(),
        ) {
            (Some(api_key), Some(to)) if !to.is_empty() => Some(SendgridChannel {
                api_key,
                to,
                from: std::env::var("WC_EMAIL_FROM")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| DEFAULT_EMAIL_FROM.to_string()),
                url: SENDGRID_URL.to_string(),
            }),
            _ => None,
        };

        let pubsub = if topic.is_empty() {
            None
        } else {
            match gcp_token().await {
                Ok(token) => Some(PubSubChannel {
                    topic: topic.to_string(),
                    base_url: PUBSUB_BASE.to_string(),
                    token,
                }),
                Err(err) => {
                    log(&format!("pubsub auth failed: {err}"));
                    None
                }
            }
        };

        Self {
            slack_webhook,
            telegram,
            sendgrid,
            pubsub,
        }
    }
}

async fn gcp_token() -> Result<String, String> {
    let auth = crate::skarbiec::gcp_provider()
        .await
        .map_err(|e| e.to_string())?;
    let token = auth
        .token(&[CLOUD_PLATFORM_SCOPE])
        .await
        .map_err(|e| e.to_string())?;
    Ok(token.as_str().to_string())
}

/// Error on a non-2xx response, including the upstream body.
async fn ensure_success(response: reqwest::Response) -> Result<(), String> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    Err(format!("HTTP {status}: {body}"))
}

async fn send_slack(client: &reqwest::Client, url: &str, message: &str) -> Result<(), String> {
    let response = client
        .post(url)
        .json(&json!({"text": message}))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    ensure_success(response).await
}

async fn send_telegram(
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

async fn send_email(
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

async fn send_pubsub(
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

/// Python `subject or message[:80]`, char-boundary safe.
fn email_subject<'a>(subject: &'a str, message: &'a str) -> String {
    if subject.is_empty() {
        message.chars().take(80).collect()
    } else {
        subject.to_string()
    }
}

/// Send an alert to every configured channel. Each channel is fault-isolated
/// (see module docs): a failure logs `[alert] <channel> failed: {err}` and
/// the remaining channels still fire.
pub async fn send_alert_with(channels: &AlertChannels, message: &str, subject: &str) {
    log(message);
    let client = reqwest::Client::new();

    if let Some(url) = &channels.slack_webhook {
        match send_slack(&client, url, message).await {
            Ok(()) => log("Slack sent"),
            Err(err) => log(&format!("slack failed: {err}")),
        }
    }
    if let Some(telegram) = &channels.telegram {
        match send_telegram(&client, telegram, message).await {
            Ok(()) => log("Telegram sent"),
            Err(err) => log(&format!("telegram failed: {err}")),
        }
    }
    if let Some(sendgrid) = &channels.sendgrid {
        let subject = email_subject(subject, message);
        match send_email(&client, sendgrid, &subject, message).await {
            Ok(()) => log("Email sent"),
            Err(err) => log(&format!("email failed: {err}")),
        }
    }
    if let Some(pubsub) = &channels.pubsub {
        match send_pubsub(&client, pubsub, message).await {
            Ok(()) => log("Pub/Sub sent"),
            Err(err) => log(&format!("pubsub failed: {err}")),
        }
    }
}

/// Send an alert to all configured channels (Python `send_alert`).
pub async fn send_alert(topic: &str, message: &str, subject: &str) {
    let channels = AlertChannels::from_env(topic).await;
    send_alert_with(&channels, message, subject).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{http_response, mock_http};

    fn ok() -> String {
        http_response(200, "OK", "{}")
    }

    #[tokio::test]
    async fn slack_posts_text_payload() {
        let mock = mock_http(vec![ok()]).await;
        let channels = AlertChannels {
            slack_webhook: Some(mock.base_url.clone()),
            ..Default::default()
        };
        send_alert_with(&channels, "disk full", "").await;
        let requests = mock.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("POST / "), "{}", requests[0]);
        assert!(
            requests[0].contains(r#"{"text":"disk full"}"#),
            "{}",
            requests[0]
        );
    }

    #[tokio::test]
    async fn telegram_posts_markdown_message() {
        let mock = mock_http(vec![ok()]).await;
        let channels = AlertChannels {
            telegram: Some(TelegramChannel {
                token: "tok123".into(),
                chat_id: "chat42".into(),
                api_base: mock.base_url.clone(),
            }),
            ..Default::default()
        };
        send_alert_with(&channels, "hello *fleet*", "").await;
        let requests = mock.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].starts_with("POST /bottok123/sendMessage "),
            "{}",
            requests[0]
        );
        assert!(
            requests[0]
                .contains(r#"{"chat_id":"chat42","text":"hello *fleet*","parse_mode":"Markdown"}"#),
            "{}",
            requests[0]
        );
    }

    #[tokio::test]
    async fn email_falls_back_to_message_prefix_subject() {
        let mock = mock_http(vec![ok()]).await;
        let long_message = "x".repeat(200);
        let channels = AlertChannels {
            sendgrid: Some(SendgridChannel {
                api_key: "SG.key".into(),
                to: "ops@example.com".into(),
                from: "compute@example.com".into(),
                url: format!("{}/v3/mail/send", mock.base_url),
            }),
            ..Default::default()
        };
        send_alert_with(&channels, &long_message, "").await;
        let requests = mock.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].contains("authorization: Bearer SG.key"),
            "{}",
            requests[0]
        );
        // Empty subject -> first 80 chars of the message.
        let expected_subject = "x".repeat(80);
        assert!(
            requests[0].contains(&format!(r#""subject":"{expected_subject}""#)),
            "{}",
            requests[0]
        );
        assert!(requests[0].contains(r#""from":{"email":"compute@example.com"}"#));
    }

    #[tokio::test]
    async fn email_uses_explicit_subject_when_given() {
        let mock = mock_http(vec![ok()]).await;
        let channels = AlertChannels {
            sendgrid: Some(SendgridChannel {
                api_key: "k".into(),
                to: "ops@example.com".into(),
                from: "compute@example.com".into(),
                url: mock.base_url.clone(),
            }),
            ..Default::default()
        };
        send_alert_with(&channels, "body text", "explicit subject").await;
        let requests = mock.requests.lock().expect("requests lock");
        assert!(
            requests[0].contains(r#""subject":"explicit subject""#),
            "{}",
            requests[0]
        );
    }

    #[tokio::test]
    async fn pubsub_publishes_base64_message() {
        let mock = mock_http(vec![ok()]).await;
        let channels = AlertChannels {
            pubsub: Some(PubSubChannel {
                topic: "projects/p/topics/t".into(),
                base_url: mock.base_url.clone(),
                token: "ya29.tok".into(),
            }),
            ..Default::default()
        };
        send_alert_with(&channels, "hello", "").await;
        let requests = mock.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].starts_with("POST /v1/projects/p/topics/t:publish "),
            "{}",
            requests[0]
        );
        assert!(
            requests[0].contains("authorization: Bearer ya29.tok"),
            "{}",
            requests[0]
        );
        // base64("hello") == "aGVsbG8="
        assert!(
            requests[0].contains(r#"{"messages":[{"data":"aGVsbG8="}]}"#),
            "{}",
            requests[0]
        );
    }

    #[tokio::test]
    async fn broken_channel_does_not_suppress_the_others() {
        let mock = mock_http(vec![ok()]).await;
        let channels = AlertChannels {
            // Port 1 refuses connections -> slack fails.
            slack_webhook: Some("http://127.0.0.1:1/webhook".into()),
            telegram: Some(TelegramChannel {
                token: "tok".into(),
                chat_id: "c".into(),
                api_base: mock.base_url.clone(),
            }),
            ..Default::default()
        };
        send_alert_with(&channels, "m", "").await;
        let requests = mock.requests.lock().expect("requests lock");
        assert_eq!(
            requests.len(),
            1,
            "telegram must still fire after slack failed"
        );
    }

    #[tokio::test]
    async fn non_2xx_is_a_channel_error_not_a_success() {
        let mock = mock_http(vec![http_response(500, "Internal Server Error", "boom")]).await;
        let channels = AlertChannels {
            slack_webhook: Some(mock.base_url.clone()),
            ..Default::default()
        };
        // Must not panic; the failure is logged and swallowed.
        send_alert_with(&channels, "m", "").await;
        let requests = mock.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn no_channels_configured_is_a_noop() {
        send_alert_with(&AlertChannels::default(), "quiet", "").await;
    }
}
