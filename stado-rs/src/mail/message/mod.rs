//! Analysis of one message Skrzynka received.
//!
//! What a message is about and whether it asks the operator to act is read
//! by a model through Oko's one-shot call (`judge-prompt.md` beside this
//! file), in any language and wording. The phrase tables this replaced
//! decided both by English substrings, so a notice worded otherwise was read
//! as asking nothing. A message the model could not read carries the cause,
//! and the watchdog shows it to the operator as unread rather than as
//! harmless. `text` and `patterns` still harvest the amounts, dates and links
//! the message spells out, which are its own structure.

mod patterns;
mod text;

use serde::Deserialize;

use super::{MailAnalysis, SkrzynkaMessage};
use crate::primitives::model_judgement;

use patterns::{AMOUNT_RE, DATE_RE, URL_RE};
use text::regex_values;

const PROMPT: &str = include_str!("judge-prompt.md");
const FROM_SLOT: &str = "{from}";
const SUBJECT_SLOT: &str = "{subject}";
const BODY_SLOT: &str = "{body}";

/// The model's reading of one message, as the prompt asks for it.
#[derive(Deserialize)]
struct Reading {
    categories: Vec<String>,
    action: Option<String>,
}

/// What the model read in `message`, or why nothing was read.
fn read(message: &SkrzynkaMessage) -> Result<Reading, String> {
    let prompt = PROMPT
        .replace(FROM_SLOT, &message.sender)
        .replace(SUBJECT_SLOT, &message.subject)
        .replace(BODY_SLOT, &message.body_text);
    let answer = model_judgement::ask(&prompt)?;
    serde_json::from_str(answer.trim())
        .map_err(|error| format!("the model's reading is not the asked JSON ({error}): {answer}"))
}

pub fn analyze(message: &SkrzynkaMessage) -> MailAnalysis {
    let combined = format!(
        "{}\n{}\n{}\n{}",
        message.subject, message.sender, message.snippet, message.body_text
    );
    let (categories, action, reading_error) = match read(message) {
        Ok(reading) => (reading.categories, reading.action, None),
        Err(cause) => (Vec::new(), None, Some(cause)),
    };

    MailAnalysis {
        id: message.id.clone(),
        mailbox_id: message.mailbox_id.clone(),
        date: match &message.sent_at {
            Some(sent) => sent.clone(),
            None => message.received_at.clone(),
        },
        received_at: message.received_at.clone(),
        from: message.sender.clone(),
        to: message.recipients.clone(),
        subject: message.subject.clone(),
        snippet: message.snippet.clone(),
        categories,
        amounts: regex_values(&AMOUNT_RE, &combined),
        date_mentions: regex_values(&DATE_RE, &combined),
        links: regex_values(&URL_RE, &combined),
        action_required: action.is_some() || reading_error.is_some(),
        action_signals: action.into_iter().collect(),
        reading_error,
    }
}
