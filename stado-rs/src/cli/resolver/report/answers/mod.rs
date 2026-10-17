//! When each service last answered a request through this resolver, and
//! since when its requests have ended unanswered.
//!
//! The waiting list (`resolver-waiting.json`) holds a request only while its
//! client waits. A client the harness ends gives its place back as soon as it
//! closes, so a service that answers nothing never holds a wait long: every
//! request is ended by its own client first, and the list only ever shows
//! young waits. Nothing published said that the same service had answered
//! none of them. Every request that ends is now recorded here per service:
//! when it last answered, and when the first and the latest request ended
//! unanswered since, so a client about to ask reads a service that takes
//! requests and answers none and refuses instead of becoming the next one.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::published::{now_iso, state_path};

const ANSWERS_FILE: &str = "resolver-answers.json";

/// What one service did with the requests that ended through this resolver.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Answers {
    /// When the service last sent the first byte of an answer; absent while
    /// it has answered none since this resolver started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_answered_at: Option<String>,
    /// When the first request since that answer ended without a first byte;
    /// absent while none has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) first_unanswered_at: Option<String>,
    /// When the latest such request ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_unanswered_at: Option<String>,
}

static ANSWERS: Mutex<BTreeMap<String, Answers>> = Mutex::new(BTreeMap::new());

/// Write the record beside the resolver's state, atomically and best effort
/// like the waiting list: a resolver that cannot write its diagnostics still
/// serves traffic, and says why on stderr.
fn publish(answers: &BTreeMap<String, Answers>) {
    let Some(path) = state_path().map(|path| path.with_file_name(ANSWERS_FILE)) else {
        return;
    };
    let body = match serde_json::to_vec_pretty(answers) {
        Ok(body) => body,
        Err(error) => {
            eprintln!(
                "stado resolver could not serialize when each service last answered: {error}"
            );
            return;
        }
    };
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    if let Err(error) = std::fs::write(&temp, &body).and_then(|()| std::fs::rename(&temp, &path)) {
        eprintln!(
            "{}",
            crate::primitives::staging::failed_with(
                format!("stado resolver could not publish when each service last answered to {}: {error}", path.display()),
                &temp,
            )
        );
    }
}

/// End the request's wait `key` in the waiting list and record it against
/// `service`, `answered` when the service sent the first byte of an answer;
/// returns how many requests still wait.
pub(crate) fn ended(service: &str, answered: bool, key: u64) -> usize {
    let still = super::waiting::end(key);
    // The record is set field by field, so a panic while it was held leaves
    // it consistent and the lock is recovered, as the waiting list does.
    let mut answers = match ANSWERS.lock() {
        Ok(answers) => answers,
        Err(poisoned) => poisoned.into_inner(),
    };
    let entry = answers.entry(service.to_string()).or_default();
    let now = now_iso();
    if answered {
        *entry = Answers {
            last_answered_at: Some(now),
            ..Answers::default()
        };
    } else {
        if entry.first_unanswered_at.is_none() {
            entry.first_unanswered_at = Some(now.clone());
        }
        entry.last_unanswered_at = Some(now);
    }
    publish(&answers);
    still
}
