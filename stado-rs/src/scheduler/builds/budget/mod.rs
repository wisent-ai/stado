//! The fleet's daily build budget: how many native build jobs this fleet
//! submits in one UTC day, counted wherever a build is enqueued.
//!
//! The workshop's rule is three builds a day, and until 2026-09-21 nothing
//! in the product held anyone to it. That day one session, refused
//! `stado release submit`, declared a recipe and ran it: two 52-minute
//! darwin builds plus a poller that enqueued another every ten minutes, on
//! a host already under memory pressure. The operator, reading the queue:
//! "czyli obszedles w ten sposob nasz ci/cd pipeline gdzie jest limit 3
//! buildow dziennie". A ceiling that lives only in an agreement is not a
//! ceiling; it is a sentence somebody remembers.
//!
//! The charge is taken where a job is submitted ([`super::charge`]), so no
//! path can spend without being counted: the release pipeline asks this
//! module first so its refusal names the release, and a raw submission
//! carrying a compile is charged anyway. The count lives in the canonical
//! registry, so every machine that can submit reads the same number and a
//! second coordinator cannot spend the budget twice. `stado queue budget`
//! reads it and declares the ceiling.

use serde_json::{json, Map, Value};

mod day;
#[cfg(test)]
mod tests;

use day::{next_day, runs_submitted_on};

/// Registry key holding the budget document.
pub const BUILD_BUDGET_KEY: &str = "build_budget";
/// Builds the fleet may submit in one UTC day when the registry declares no
/// other ceiling. Three is the workshop's standing rule.
pub const DEFAULT_DAILY_BUILD_LIMIT: u64 = 3;
/// A ceiling of zero stops every build; there is no "unlimited" value,
/// because an unlimited budget is what this module exists to end.
const DAY_FORMAT: &str = "%Y-%m-%d";

/// What the fleet has spent today and what it may spend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildBudget {
    /// The UTC day the count belongs to.
    pub day: String,
    /// Build jobs already submitted on that day.
    pub used: u64,
    /// Build jobs the fleet may submit on one day.
    pub limit: u64,
    /// The submissions already charged today, by the key the charging path
    /// knows before the job exists: its run id. Two paths may meet the same
    /// build — the client that submits it and the worker that claims it —
    /// and a day must be charged for that build once, so the key decides
    /// rather than the order they arrive in.
    pub charged: Vec<String>,
}

impl BuildBudget {
    /// The budget as of `now`, from the registry document. A count from an
    /// earlier day is spent history: the day rolls and the count starts at
    /// zero, which is why the day is stored beside it rather than a bare
    /// counter that would have to be reset by somebody.
    pub fn read(document: &Value, now: chrono::DateTime<chrono::Utc>) -> Self {
        let today = now.format(DAY_FORMAT).to_string();
        let entry = document.get(BUILD_BUDGET_KEY);
        let limit = entry
            .and_then(|budget| budget.get("limit"))
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_DAILY_BUILD_LIMIT);
        let counted = entry
            .filter(|budget| budget.get("day").and_then(Value::as_str) == Some(today.as_str()))
            .and_then(|budget| budget.get("used"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        // A counter that begins the day it ships forgives every build the
        // fleet already made, and a lost write forgives the ones it lost.
        // The recipes carry their own runs with the instant each was
        // submitted, so the day's floor is observable: whichever is higher
        // is the truth, and the count can only ever be understated by runs
        // the registry itself no longer holds.
        let used = counted.max(runs_submitted_on(document, &today));
        let charged = entry
            .filter(|budget| budget.get("day").and_then(Value::as_str) == Some(today.as_str()))
            .and_then(|budget| budget.get("charged"))
            .and_then(Value::as_array)
            .map(|keys| {
                keys.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            day: today,
            used,
            limit,
            charged,
        }
    }

    /// Whether this submission has already been charged to today.
    pub fn already_charged(&self, key: &str) -> bool {
        self.charged.iter().any(|charged| charged == key)
    }

    /// What is left today.
    pub fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.used)
    }

    /// Why `wanted` more builds cannot be submitted, if they cannot.
    ///
    /// The sentence carries the three numbers a reader needs — spent,
    /// ceiling, asked for — the moment the day rolls over, and the two
    /// commands that change the answer, because a refusal that names no
    /// route is worked around, which is exactly how this budget was spent.
    pub fn refusal(&self, wanted: usize, reason: &str) -> Option<String> {
        let wanted = wanted as u64;
        if wanted <= self.remaining() {
            return None;
        }
        Some(format!(
            "the fleet's daily build budget is spent: {} of {} build job(s) submitted on {} (UTC), \
             and {reason} asks for {wanted} more. The count resets at {}T00:00:00Z. Read it with \
             `stado queue budget`, and declare a different ceiling — deliberately, once — with \
             `stado queue budget --limit <N>`.",
            self.used,
            self.limit,
            self.day,
            next_day(&self.day)
        ))
    }

    /// Record `submitted` more builds against today's count, in the document
    /// that is about to be written under the registry's fence. `keys` are the
    /// submissions this charge belongs to, kept so the same build reaching a
    /// second charging path is not charged twice.
    pub fn record(&self, document: &mut Value, submitted: usize, keys: &[String]) {
        if submitted == 0 {
            return;
        }
        let Some(object) = document.as_object_mut() else {
            return;
        };
        let spent = self.used.saturating_add(submitted as u64);
        let mut charged = self.charged.clone();
        for key in keys {
            if !charged.iter().any(|seen| seen == key) {
                charged.push(key.clone());
            }
        }
        let entry = object
            .entry(BUILD_BUDGET_KEY.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        *entry = json!({
            "day": self.day,
            "used": spent,
            "limit": self.limit,
            "charged": charged,
        });
    }

    /// Declare a different ceiling in the document, keeping today's count.
    pub fn with_limit(&self, document: &mut Value, limit: u64) {
        let Some(object) = document.as_object_mut() else {
            return;
        };
        object.insert(
            BUILD_BUDGET_KEY.to_string(),
            json!({
                "day": self.day,
                "used": self.used,
                "limit": limit,
                "charged": self.charged,
            }),
        );
    }
}
