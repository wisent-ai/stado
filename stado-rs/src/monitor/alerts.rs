//! Optional Slack, Telegram, SendGrid, most (SMS), and GCP Pub/Sub alert
//! delivery. `alerts.channels` is the explicit enablement fence: with no
//! enabled channels, dispatch performs no credential or network lookup, and
//! each delivery is fault-isolated with a bounded structured failure line.

use serde_json::Value;

mod send;
#[cfg(test)]
mod tests;

use send::{email_subject, send_email, send_most, send_pubsub, send_slack, send_telegram};

/// GCP OAuth scope for the Pub/Sub publish call.
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
/// Telegram Bot API base (overridable per-channel for tests).
const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
/// SendGrid mail-send endpoint.
const SENDGRID_URL: &str = "https://api.sendgrid.com/v3/mail/send";
/// Pub/Sub REST base.
const PUBSUB_BASE: &str = "https://pubsub.googleapis.com";
/// Twilio REST base for the most (SMS) channel.
const TWILIO_API_BASE: &str = "https://api.twilio.com";
/// Python `WC_EMAIL_FROM` default.
const DEFAULT_EMAIL_FROM: &str = "compute@example.com";

fn log(msg: &str) {
    eprintln!("[alert] {msg}");
}

/// One channel could not deliver. Logged twice on purpose and fatal never:
/// the `[alert]` line is what a human tailing the monitor reads, and the
/// structured line is what a log query finds a week later.
fn channel_failed(channel: &str, error: &str) {
    let code = crate::failure::classify_message(error);
    tracing::error!(
        failure_point = "monitor.alerts.deliver",
        error_code = code.as_str(),
        service = "alerts",
        retryable = code.retryable(),
        severity = code.severity().as_str(),
        channel = channel,
        detail = %crate::failure::bounded_detail(error),
        "alert channel delivery failed; the remaining channels still fire"
    );
    log(&format!("{channel} failed: {error}"));
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

/// most (SMS) channel: destination from `stado-alerts/most_phone`, Twilio
/// credentials resolved from `most-twilio` through the `most` integration
/// provider grant, delivered in-process so the alert path never depends on
/// the dashboard it may be alerting about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MostChannel {
    pub phone: String,
    pub account_sid: String,
    pub auth_token: String,
    pub api_version: String,
    pub messaging_service_sid: Option<String>,
    pub from_number: Option<String>,
    /// Twilio REST base; tests point it at the loopback mock.
    pub api_base: String,
}

/// Resolved alert-channel configuration; channels with no config are skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlertChannels {
    /// Slack webhook URL from `stado-alerts/slack_webhook` in Skarbiec.
    pub slack_webhook: Option<String>,
    pub telegram: Option<TelegramChannel>,
    pub sendgrid: Option<SendgridChannel>,
    pub pubsub: Option<PubSubChannel>,
    pub most: Option<MostChannel>,
}

impl AlertChannels {
    /// Resolve only explicitly enabled alert channels.
    pub async fn from_env(topic: &str) -> Self {
        let enabled = crate::config::alert_channels();
        let is_enabled = |channel: &str| enabled.iter().any(|value| value == channel);
        if enabled.is_empty() {
            return Self::default();
        }

        let needs_stored = is_enabled("slack")
            || is_enabled("telegram")
            || is_enabled("sendgrid")
            || is_enabled("most");
        let stored = if needs_stored {
            match crate::skarbiec::Client::configured() {
                Ok(vault) => match vault.read_item("stado-alerts").await {
                    Ok(value) => value,
                    Err(err) => {
                        channel_failed("configuration", &err.to_string());
                        Value::Null
                    }
                },
                Err(err) => {
                    channel_failed("configuration", &err.to_string());
                    Value::Null
                }
            }
        } else {
            Value::Null
        };
        let secret = |field: &str| {
            stored
                .get(field)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };

        let slack_webhook = is_enabled("slack").then(|| secret("slack_webhook")).flatten();
        let telegram = if is_enabled("telegram") {
            match (secret("telegram_bot_token"), secret("telegram_chat_id")) {
                (Some(token), Some(chat_id)) => Some(TelegramChannel {
                    token,
                    chat_id,
                    api_base: TELEGRAM_API_BASE.to_string(),
                }),
                _ => None,
            }
        } else {
            None
        };
        let sendgrid = if is_enabled("sendgrid") {
            match (
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
            }
        } else {
            None
        };
        let most = if is_enabled("most") {
            resolve_most(secret("most_phone")).await
        } else {
            None
        };
        let pubsub = if is_enabled("gcp-pubsub") && !topic.is_empty() {
            match gcp_token().await {
                Ok(token) => Some(PubSubChannel {
                    topic: topic.to_string(),
                    base_url: PUBSUB_BASE.to_string(),
                    token,
                }),
                Err(err) => {
                    channel_failed("pubsub-auth", &err);
                    None
                }
            }
        } else {
            None
        };

        Self {
            slack_webhook,
            telegram,
            sendgrid,
            pubsub,
            most,
        }
    }
}

/// Resolve the most (SMS) channel: destination from the alerts item, Twilio
/// material through the `most` integration provider grant. Any gap degrades
/// to no channel with a structured failure line, never a panic.
async fn resolve_most(phone: Option<String>) -> Option<MostChannel> {
    let phone = phone.filter(|value| !value.is_empty())?;
    let provider = crate::skarbiec::Client::integration_provider("most")
        .map_err(|err| channel_failed("most-configuration", &err.to_string()))
        .ok()?;
    let item = provider
        .read_item("most-twilio")
        .await
        .map_err(|err| channel_failed("most-configuration", &err.to_string()))
        .ok()?;
    let field = |name: &str| {
        item.get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    match (field("account_sid"), field("auth_token"), field("api_version")) {
        (Some(account_sid), Some(auth_token), Some(api_version)) => Some(MostChannel {
            phone,
            account_sid,
            auth_token,
            api_version,
            messaging_service_sid: field("messaging_service_sid"),
            from_number: field("from_number"),
            api_base: TWILIO_API_BASE.to_string(),
        }),
        _ => {
            channel_failed(
                "most-configuration",
                "most-twilio needs account_sid, auth_token, and api_version",
            );
            None
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

/// Send an alert to every configured channel. Each channel is fault-isolated
/// (see module docs): a failure goes through [`channel_failed`] and the
/// remaining channels still fire.
pub async fn send_alert_with(channels: &AlertChannels, message: &str, subject: &str) {
    log(message);
    let client = reqwest::Client::new();

    if let Some(url) = &channels.slack_webhook {
        match send_slack(&client, url, message).await {
            Ok(()) => log("Slack sent"),
            Err(err) => channel_failed("slack", &err),
        }
    }
    if let Some(telegram) = &channels.telegram {
        match send_telegram(&client, telegram, message).await {
            Ok(()) => log("Telegram sent"),
            Err(err) => channel_failed("telegram", &err),
        }
    }
    if let Some(sendgrid) = &channels.sendgrid {
        let subject = email_subject(subject, message);
        match send_email(&client, sendgrid, &subject, message).await {
            Ok(()) => log("Email sent"),
            Err(err) => channel_failed("email", &err),
        }
    }
    if let Some(most) = &channels.most {
        match send_most(&client, most, message).await {
            Ok(()) => log("SMS sent"),
            Err(err) => channel_failed("most", &err),
        }
    }
    if let Some(pubsub) = &channels.pubsub {
        match send_pubsub(&client, pubsub, message).await {
            Ok(()) => log("Pub/Sub sent"),
            Err(err) => channel_failed("pubsub", &err),
        }
    }
}

/// Send an alert to all explicitly enabled channels.
pub async fn send_alert(topic: &str, message: &str, subject: &str) {
    let channels = AlertChannels::from_env(topic).await;
    send_alert_with(&channels, message, subject).await;
}
