//! Deterministic analysis of one message Skrzynka received.
//!
//! The rule tables below are the whole classifier: `text` harvests the
//! message text and `patterns` holds the compiled scanners. Skrzynka has
//! already turned the message into plain text.

mod patterns;
mod text;

use super::{MailAnalysis, SkrzynkaMessage};

use patterns::{AMOUNT_RE, DATE_RE, URL_RE};
use text::regex_values;

pub fn analyze(message: &SkrzynkaMessage) -> MailAnalysis {
    let combined = format!(
        "{}\n{}\n{}\n{}",
        message.subject, message.sender, message.snippet, message.body_text
    );
    let lowered = combined.to_ascii_lowercase();

    let category_rules: &[(&str, &[&str])] = &[
        ("azure", &["azure", "microsoft cloud"]),
        (
            "startup_program",
            &[
                "microsoft for startups",
                "founders hub",
                "startup sponsorship",
            ],
        ),
        ("credits", &["credit", "sponsorship", "grant"]),
        (
            "billing",
            &["billing", "balance", "cost management", "payment"],
        ),
        ("invoice", &["invoice", "receipt"]),
        ("quota", &["quota", "capacity request", "service limit"]),
        (
            "security",
            &["security alert", "password", "verification code", "sign-in"],
        ),
    ];
    let categories = category_rules
        .iter()
        .filter(|(_, needles)| needles.iter().any(|needle| lowered.contains(*needle)))
        .map(|(category, _)| (*category).to_string())
        .collect();

    let action_rules = [
        "action required",
        "activate your",
        "redeem",
        "sign the",
        "sign in to",
        "verify your",
        "respond by",
        "expires",
        "deadline",
    ];
    let action_signals = action_rules
        .into_iter()
        .filter(|signal| lowered.contains(signal))
        .map(str::to_string)
        .collect::<Vec<_>>();

    MailAnalysis {
        id: message.id.clone(),
        mailbox_id: message.mailbox_id.clone(),
        date: message
            .sent_at
            .clone()
            .unwrap_or_else(|| message.received_at.clone()),
        received_at: message.received_at.clone(),
        from: message.sender.clone(),
        to: message.recipients.clone(),
        subject: message.subject.clone(),
        snippet: message.snippet.clone(),
        categories,
        amounts: regex_values(&AMOUNT_RE, &combined),
        date_mentions: regex_values(&DATE_RE, &combined),
        links: regex_values(&URL_RE, &combined),
        action_required: !action_signals.is_empty(),
        action_signals,
    }
}
