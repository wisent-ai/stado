//! Deterministic analysis of one Gmail message document.
//!
//! The rule tables below are the whole classifier: `headers` reads the
//! published header names, `body` reconstructs the text, `text` flattens
//! and harvests it, and `patterns` holds the compiled scanners.

mod body;
mod headers;
mod patterns;
mod text;

use serde_json::Value;

use super::MailAnalysis;

use body::extract_body;
use headers::header;
use patterns::{AMOUNT_RE, DATE_RE, URL_RE};
use text::{html_to_text, regex_values};

pub(super) fn analyze_message(message: &Value) -> MailAnalysis {
    let payload = message.get("payload").unwrap_or(&Value::Null);
    let plain = extract_body(payload, "text/plain");
    let html = extract_body(payload, "text/html");
    let body = if plain.trim().is_empty() {
        html_to_text(&html)
    } else {
        plain
    };
    let subject = header(payload, "subject");
    let from = header(payload, "from");
    let to = header(payload, "to");
    let date = header(payload, "date");
    let snippet = message
        .get("snippet")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let combined = format!("{subject}\n{from}\n{snippet}\n{body}");
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

    let id = message
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    MailAnalysis {
        gmail_url: format!("https://mail.google.com/mail/u/me/#all/{id}"),
        id,
        thread_id: message
            .get("threadId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        date,
        internal_date: message
            .get("internalDate")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|value| value.to_rfc3339()),
        from,
        to,
        subject,
        labels: message
            .get("labelIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        snippet,
        categories,
        amounts: regex_values(&AMOUNT_RE, &combined),
        date_mentions: regex_values(&DATE_RE, &combined),
        links: regex_values(&URL_RE, &combined),
        action_required: !action_signals.is_empty(),
        action_signals,
    }
}
