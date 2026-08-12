//! Optional Slack, Telegram, SendGrid, Resend, most (SMS), and GCP Pub/Sub
//! alert delivery. `alerts.channels` is the explicit enablement fence: with no
//! enabled channels, dispatch performs no credential or network lookup, and
//! each delivery is fault-isolated with a bounded structured failure line.

mod send;
#[cfg(test)]
mod tests;

pub(crate) use send::resend_verified_domains;
use send::{
    email_subject, send_email, send_most, send_pubsub, send_resend_email, send_slack, send_telegram,
};

/// GCP OAuth scope for the Pub/Sub publish call.
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
/// Telegram Bot API base (overridable per-channel for tests).
const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
/// SendGrid mail-send endpoint.
const SENDGRID_URL: &str = "https://api.sendgrid.com/v3/mail/send";
/// Resend mail-send endpoint.
const RESEND_URL: &str = "https://api.resend.com/emails";
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

/// Resend channel config. The key is this deployment's own `RESEND_API_KEY`
/// vault item rather than a copy inside `stado-alerts`: one secret, one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResendChannel {
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
    pub resend: Option<ResendChannel>,
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
            || is_enabled("resend")
            || is_enabled("most");
        let stored: std::collections::BTreeMap<String, String> = if needs_stored {
            // Per field, not the whole item: the listener refuses a read that
            // names no field ("field required"), so asking for the item as a
            // whole silently emptied every channel and the operator was left
            // with an alerting path that resolved to nothing.
            let mut wanted: Vec<&str> = Vec::new();
            if is_enabled("slack") {
                wanted.push("slack_webhook");
            }
            if is_enabled("telegram") {
                wanted.push("telegram_bot_token");
                wanted.push("telegram_chat_id");
            }
            if is_enabled("sendgrid") {
                wanted.push("sendgrid_api_key");
            }
            if is_enabled("resend") {
                // Only ask the vault for what the config document does not
                // already answer: a deployment that names its destination in
                // config holds no such field, and the read would be refused.
                if crate::config::alert_email_to().is_empty() {
                    wanted.push("email_to");
                }
                if crate::config::alert_email_from().is_empty() {
                    wanted.push("email_from");
                }
            }
            if is_enabled("most") {
                wanted.push("most_phone");
            }
            match crate::skarbiec::Client::configured() {
                Ok(vault) => {
                    let mut found = std::collections::BTreeMap::new();
                    for field in wanted {
                        match vault.read_string("stado-alerts", field).await {
                            Ok(Some(value)) if !value.is_empty() => {
                                found.insert(field.to_string(), value);
                            }
                            Ok(_) => {}
                            Err(err) => channel_failed("configuration", &err.to_string()),
                        }
                    }
                    found
                }
                Err(err) => {
                    channel_failed("configuration", &err.to_string());
                    std::collections::BTreeMap::new()
                }
            }
        } else {
            std::collections::BTreeMap::new()
        };
        let secret = |field: &str| stored.get(field).cloned();

        let slack_webhook = is_enabled("slack")
            .then(|| secret("slack_webhook"))
            .flatten();
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
        let resend = if is_enabled("resend") {
            resolve_resend(secret("email_to"), secret("email_from")).await
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
            resend,
            pubsub,
            most,
        }
    }
}

/// Resolve the Resend email channel: destination and sender from the config
/// document (`alerts.email_to`, `alerts.email_from`, or their `WC_EMAIL_*`
/// env names), falling back to the alerts item for a deployment that keeps
/// them in the vault; key from the vault item named by `alerts.resend_item`.
/// A gap degrades to no channel with a structured failure line.
async fn resolve_resend(to: Option<String>, from: Option<String>) -> Option<ResendChannel> {
    let configured = |value: &str| (!value.is_empty()).then(|| value.to_string());
    let Some(to) = configured(crate::config::alert_email_to())
        .or(to)
        .filter(|value| !value.is_empty())
    else {
        channel_failed(
            "resend-configuration",
            "no destination: set alerts.email_to (or WC_EMAIL_TO)",
        );
        return None;
    };
    // The coordinator's grant does not carry the resend key, and reading with
    // it turned the only configured channel into no channel at all.
    let vault = crate::skarbiec::Client::alert_key_reader()
        .map_err(|err| channel_failed("resend-configuration", &err.to_string()))
        .ok()?;
    let item = crate::config::alert_resend_item();
    let field = crate::config::alert_resend_field();
    let key = match vault.read_string(item, field).await {
        Ok(Some(value)) if !value.is_empty() => value,
        Ok(_) => {
            channel_failed(
                "resend-configuration",
                &format!("{item}/{field} is empty; point alerts.resend_item at the live key"),
            );
            return None;
        }
        Err(err) => {
            channel_failed("resend-configuration", &err.to_string());
            return None;
        }
    };
    Some(ResendChannel {
        api_key: key,
        to,
        from: configured(crate::config::alert_email_from())
            .or(from)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_EMAIL_FROM.to_string()),
        url: RESEND_URL.to_string(),
    })
}

/// Resolve the most (SMS) channel: destination from the alerts item, Twilio
/// material through the `most` integration provider grant. Any gap degrades
/// to no channel with a structured failure line, never a panic.
async fn resolve_most(phone: Option<String>) -> Option<MostChannel> {
    let phone = phone.filter(|value| !value.is_empty())?;
    let provider = crate::skarbiec::Client::integration_provider("most")
        .map_err(|err| channel_failed("most-configuration", &err.to_string()))
        .ok()?;
    // Field by field: the listener refuses a read that names no field, so the
    // whole-item form resolved to no channel and the deployment believed it
    // had no way to page anyone.
    let mut values = std::collections::BTreeMap::new();
    for name in [
        "account_sid",
        "auth_token",
        "api_version",
        "messaging_service_sid",
        "from_number",
    ] {
        match provider.read_string("most-twilio", name).await {
            Ok(Some(value)) if !value.is_empty() => {
                values.insert(name, value);
            }
            Ok(_) => {}
            Err(err) => {
                channel_failed("most-configuration", &err.to_string());
                return None;
            }
        }
    }
    let field = |name: &str| values.get(name).cloned();
    match (
        field("account_sid"),
        field("auth_token"),
        field("api_version"),
    ) {
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
            // Name what the grant actually returned: "needs three fields" hid
            // which read came back empty and cost a day of guessing.
            let missing: Vec<&str> = ["account_sid", "auth_token", "api_version"]
                .into_iter()
                .filter(|name| !values.contains_key(name))
                .collect();
            channel_failed(
                "most-configuration",
                &format!(
                    "most-twilio resolved {} of 5 fields; missing {}",
                    values.len(),
                    missing.join(", ")
                ),
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
    if let Some(resend) = &channels.resend {
        let subject = email_subject(subject, message);
        match send_resend_email(&client, resend, &subject, message).await {
            Ok(()) => log("Email sent"),
            Err(err) => channel_failed("resend", &err),
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
