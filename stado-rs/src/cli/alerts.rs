//! `stado alerts` — the operator surface over [`crate::monitor::alerts`].
//!
//! `doctor` reports whether a channel *resolves*; nothing could make one
//! deliver on purpose. So a deployment could pass preflight with a key the
//! provider has since revoked, or a sender domain the provider will not
//! accept, and the first time anyone learned that was during an incident.
//!
//! `channels` prints which channels resolved and where each one would land.
//! `send` fans one message out through exactly those channels and reports
//! per-channel success, which is the only evidence that paging works.

use clap::Subcommand;

use crate::config;
use crate::monitor::alerts::{send_alert_with, AlertChannels};

use super::CmdError;

#[derive(Subcommand)]
pub enum AlertsCommands {
    /// Show which alert channels resolve, and their destinations.
    Channels {
        #[arg(long)]
        json: bool,
    },
    /// Page every configured channel with one message.
    Send {
        /// Message body.
        message: String,
        /// Subject for channels that carry one (email).
        #[arg(long, default_value = "")]
        subject: String,
    },
}

pub async fn dispatch(cmd: AlertsCommands) -> Result<(), CmdError> {
    match cmd {
        AlertsCommands::Channels { json } => channels(json).await,
        AlertsCommands::Send { message, subject } => send(&message, &subject).await,
    }
}

/// Resolved channels and where each would deliver. Secrets never appear: a
/// channel is described by its destination, which is the thing an operator
/// needs to recognise as theirs.
async fn channels(json: bool) -> Result<(), CmdError> {
    let resolved = AlertChannels::from_env(config::alerts_topic()).await;
    let mut rows: Vec<(String, String)> = Vec::new();
    if resolved.slack_webhook.is_some() {
        rows.push(("slack".to_string(), "configured webhook".to_string()));
    }
    if let Some(telegram) = &resolved.telegram {
        rows.push(("telegram".to_string(), format!("chat {}", telegram.chat_id)));
    }
    if let Some(sendgrid) = &resolved.sendgrid {
        rows.push(("sendgrid".to_string(), sendgrid.to.clone()));
    }
    if let Some(resend) = &resolved.resend {
        rows.push((
            "resend".to_string(),
            format!("{} from {}", resend.to, resend.from),
        ));
    }
    if let Some(most) = &resolved.most {
        rows.push(("most".to_string(), most.phone.clone()));
    }
    if let Some(pubsub) = &resolved.pubsub {
        rows.push(("gcp-pubsub".to_string(), pubsub.topic.clone()));
    }

    if json {
        let document: serde_json::Map<String, serde_json::Value> = rows
            .into_iter()
            .map(|(channel, destination)| (channel, serde_json::Value::from(destination)))
            .collect();
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "no alert channel resolves; enabled: [{}]",
            config::alert_channels().join(",")
        );
        return Ok(());
    }
    for (channel, destination) in rows {
        println!("{channel}\t{destination}");
    }
    Ok(())
}

/// Deliver one alert now. Every per-channel outcome is printed by the alert
/// path itself as an `[alert]` line, so a silent failure is impossible.
async fn send(message: &str, subject: &str) -> Result<(), CmdError> {
    if message.trim().is_empty() {
        return Err(CmdError::click("alerts send needs a message"));
    }
    let resolved = AlertChannels::from_env(config::alerts_topic()).await;
    send_alert_with(&resolved, message, subject).await;
    Ok(())
}
