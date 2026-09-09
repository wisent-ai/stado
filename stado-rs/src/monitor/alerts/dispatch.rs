//! Fan-out over a resolved channel set. The fault isolation the module docs
//! promise is applied here rather than inside the delivery functions: every
//! arm below matches on its own result, so a channel that cannot deliver
//! costs one failure line and none of the channels after it.

use super::send::{
    email_subject, send_email, send_most, send_pubsub, send_resend_email, send_slack, send_telegram,
};
use super::{channel_failed, log, AlertChannels};

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
