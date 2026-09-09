//! The published shapes of one analyzed message and of a whole sweep.
//!
//! `summarize` is the only aggregation step: it counts categories, keeps the
//! first sighting of every amount, and moves the messages into the report.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct MailAnalysis {
    pub id: String,
    pub thread_id: String,
    pub gmail_url: String,
    pub date: String,
    pub internal_date: Option<String>,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub labels: Vec<String>,
    pub snippet: String,
    pub categories: Vec<String>,
    pub amounts: Vec<String>,
    pub date_mentions: Vec<String>,
    pub links: Vec<String>,
    pub action_required: bool,
    pub action_signals: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MailAnalysisReport {
    pub query: String,
    pub message_count: usize,
    pub action_required_count: usize,
    pub categories: BTreeMap<String, usize>,
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
                .and_modify(|count| *count += usize::from(true))
                .or_insert(usize::from(true));
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
