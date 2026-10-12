//! The published shapes of one analyzed message and of a whole sweep.
//!
//! `summarize` is the only aggregation step: it counts categories, keeps the
//! first sighting of every amount, and moves the messages into the report.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct MailAnalysis {
    /// Skrzynka's message id: `skrzynka message show <id>` opens it.
    pub id: String,
    pub mailbox_id: String,
    pub date: String,
    pub received_at: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub snippet: String,
    pub categories: Vec<String>,
    pub amounts: Vec<String>,
    pub date_mentions: Vec<String>,
    pub links: Vec<String>,
    /// The operator has to act on it: the model read an action in it, or
    /// nobody could read it and he has to read it himself.
    pub action_required: bool,
    /// The model's sentence naming what the operator must do and when.
    pub action_signals: Vec<String>,
    /// Why the message could not be read, when it could not.
    pub reading_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MailAnalysisReport {
    pub query: String,
    pub message_count: usize,
    pub action_required_count: usize,
    pub categories: BTreeMap<String, Vec<String>>,
    pub amounts: Vec<String>,
    pub messages: Vec<MailAnalysis>,
}

pub fn summarize(query: &str, messages: Vec<MailAnalysis>) -> MailAnalysisReport {
    let mut categories = BTreeMap::new();
    let mut amounts = Vec::new();
    let mut seen_amounts = HashSet::new();
    for message in &messages {
        for category in &message.categories {
            categories
                .entry(category.clone())
                .or_insert_with(Vec::new)
                .push(message.id.clone());
        }
        for amount in &message.amounts {
            if seen_amounts.insert(amount.clone()) {
                amounts.push(amount.clone());
            }
        }
    }
    MailAnalysisReport {
        query: query.to_string(),
        message_count: messages.len(),
        action_required_count: messages
            .iter()
            .filter(|message| message.action_required)
            .count(),
        categories,
        amounts,
        messages,
    }
}
