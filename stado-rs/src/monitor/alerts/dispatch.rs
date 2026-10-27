//! Fan-out over a resolved channel set. The fault isolation the module docs
//! promise is applied here rather than inside the delivery functions: every
//! arm below matches on its own result, so a channel that cannot deliver
//! costs one failure line and none of the channels after it.
//!
//! The fan-out also answers the caller. A fan-out whose every outcome goes
//! only to the `[alert]` stderr sink exits zero whether the mail left or
//! every channel refused, and a program that pages an operator through it
//! records "the operator was told" from an exit status that cannot say
//! otherwise. The report below is what makes that claim checkable.

use super::send::{email_subject, send_most, send_pubsub, send_skrzynka, send_slack, send_telegram};
use super::{channel_failed, log, AlertChannels};

/// What one channel did with one alert.
#[derive(Debug, Clone)]
pub struct AlertDelivery {
    /// The channel name the operator configured.
    pub channel: &'static str,
    /// Whether the provider accepted the message.
    pub delivered: bool,
    /// The provider's refusal, or the destination it accepted for.
    pub detail: String,
}

/// Every channel's outcome for one alert, in the order they were tried.
pub type AlertReport = Vec<AlertDelivery>;

/// Send an alert to every configured channel. Each channel is fault-isolated
/// (see module docs): a failure goes through [`channel_failed`] and the
/// remaining channels still fire. The returned report says, per channel,
/// whether the provider took it.
pub async fn send_alert_with(
    channels: &AlertChannels,
    message: &str,
    subject: &str,
) -> AlertReport {
    log(message);
    let client = reqwest::Client::new();
    let mut report = AlertReport::new();

    if let Some(url) = &channels.slack_webhook {
        match send_slack(&client, url, message).await {
            Ok(()) => {
                log("Slack sent");
                report.push(delivered("slack", "accepted by the webhook"));
            }
            Err(err) => {
                channel_failed("slack", &err);
                report.push(refused("slack", &err));
            }
        }
    }
    if let Some(telegram) = &channels.telegram {
        match send_telegram(&client, telegram, message).await {
            Ok(()) => {
                log("Telegram sent");
                report.push(delivered("telegram", &telegram.chat_id));
            }
            Err(err) => {
                channel_failed("telegram", &err);
                report.push(refused("telegram", &err));
            }
        }
    }
    if let Some(skrzynka) = &channels.skrzynka {
        let subject = email_subject(subject, message);
        match send_skrzynka(skrzynka, &subject, message).await {
            Ok(outbound) => {
                log("Email sent through Skrzynka");
                report.push(delivered(
                    "skrzynka",
                    &format!("{} (skrzynka outbound {outbound})", skrzynka.to),
                ));
            }
            Err(err) => {
                channel_failed("skrzynka", &err);
                report.push(refused("skrzynka", &err));
            }
        }
    }
    if let Some(most) = &channels.most {
        match send_most(&client, most, message).await {
            Ok(()) => {
                log("SMS sent");
                report.push(delivered("most", &most.phone));
            }
            Err(err) => {
                channel_failed("most", &err);
                report.push(refused("most", &err));
            }
        }
    }
    if let Some(pubsub) = &channels.pubsub {
        match send_pubsub(&client, pubsub, message).await {
            Ok(()) => {
                log("Pub/Sub sent");
                report.push(delivered("pubsub", &pubsub.topic));
            }
            Err(err) => {
                channel_failed("pubsub", &err);
                report.push(refused("pubsub", &err));
            }
        }
    }
    report
}

fn delivered(channel: &'static str, destination: &str) -> AlertDelivery {
    AlertDelivery {
        channel,
        delivered: true,
        detail: destination.to_string(),
    }
}

fn refused(channel: &'static str, error: &str) -> AlertDelivery {
    AlertDelivery {
        channel,
        delivered: false,
        detail: error.trim().to_string(),
    }
}

/// Send an alert to all explicitly enabled channels.
pub async fn send_alert(topic: &str, message: &str, subject: &str) -> AlertReport {
    let channels = AlertChannels::from_env(topic).await;
    send_alert_with(&channels, message, subject).await
}
