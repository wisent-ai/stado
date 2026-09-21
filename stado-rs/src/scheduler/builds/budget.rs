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
//! Both paths that submit a build ask this module first and record through
//! it afterwards, inside the same compare-and-swap fence that records the
//! run: the recipe poller ([`super::enqueue`]) and `stado builds run`
//! ([`crate::cli::builds`]). The count lives in the canonical registry
//! beside the recipes, so every machine that can submit reads the same
//! number, and a second coordinator cannot spend the budget twice.

use serde_json::{json, Map, Value};

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
        let used = entry
            .filter(|budget| budget.get("day").and_then(Value::as_str) == Some(today.as_str()))
            .and_then(|budget| budget.get("used"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        Self {
            day: today,
            used,
            limit,
        }
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
             `stado builds budget`, and declare a different ceiling — deliberately, once — with \
             `stado builds budget --limit <N>`.",
            self.used,
            self.limit,
            self.day,
            next_day(&self.day)
        ))
    }

    /// Record `submitted` more builds against today's count, in the document
    /// that is about to be written under the registry's fence.
    pub fn record(&self, document: &mut Value, submitted: usize) {
        if submitted == 0 {
            return;
        }
        let Some(object) = document.as_object_mut() else {
            return;
        };
        let spent = self.used.saturating_add(submitted as u64);
        let entry = object
            .entry(BUILD_BUDGET_KEY.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        *entry = json!({
            "day": self.day,
            "used": spent,
            "limit": self.limit,
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
            }),
        );
    }
}

/// The day after `day`, for the sentence that says when the count resets.
/// A day string the registry holds that does not parse is reported as it
/// is rather than guessed at: a wrong reset time reads as a product fault.
fn next_day(day: &str) -> String {
    chrono::NaiveDate::parse_from_str(day, DAY_FORMAT)
        .ok()
        .and_then(|date| date.succ_opt())
        .map(|next| next.format(DAY_FORMAT).to_string())
        .unwrap_or_else(|| day.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(&format!("{day}T12:00:00Z"))
            .expect("a test instant")
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn an_undeclared_budget_is_the_workshops_three_builds_a_day() {
        let budget = BuildBudget::read(&json!({}), at("2026-09-21"));
        assert_eq!(budget.limit, DEFAULT_DAILY_BUILD_LIMIT);
        assert_eq!(budget.used, 0);
        assert!(budget.refusal(3, "a recipe").is_none());
        let refusal = budget.refusal(4, "a recipe").expect("four exceeds three");
        assert!(refusal.contains("0 of 3"), "{refusal}");
        assert!(refusal.contains("stado builds budget --limit"), "{refusal}");
    }

    #[test]
    fn yesterdays_count_does_not_spend_todays_budget() {
        let document = json!({ "build_budget": { "day": "2026-09-20", "used": 3, "limit": 3 } });
        let budget = BuildBudget::read(&document, at("2026-09-21"));
        assert_eq!(budget.used, 0, "the day rolled: {budget:?}");
        assert_eq!(budget.day, "2026-09-21");
        assert_eq!(budget.limit, 3, "the declared ceiling survives the roll");
    }

    #[test]
    fn a_spent_budget_refuses_and_names_the_reset() {
        let document = json!({ "build_budget": { "day": "2026-09-21", "used": 3, "limit": 3 } });
        let budget = BuildBudget::read(&document, at("2026-09-21"));
        assert_eq!(budget.remaining(), 0);
        let refusal = budget
            .refusal(1, "stado builds run")
            .expect("nothing is left");
        assert!(refusal.contains("3 of 3"), "{refusal}");
        assert!(refusal.contains("2026-09-22T00:00:00Z"), "{refusal}");
        assert!(
            refusal.contains("stado builds run asks for 1 more"),
            "{refusal}"
        );
    }

    #[test]
    fn a_submission_is_counted_and_a_new_ceiling_keeps_the_count() {
        let mut document = json!({ "builds": [] });
        let budget = BuildBudget::read(&document, at("2026-09-21"));
        budget.record(&mut document, 2);
        let after = BuildBudget::read(&document, at("2026-09-21"));
        assert_eq!(after.used, 2);
        assert_eq!(after.remaining(), 1);

        after.with_limit(&mut document, 6);
        let raised = BuildBudget::read(&document, at("2026-09-21"));
        assert_eq!(raised.limit, 6, "the ceiling changed");
        assert_eq!(raised.used, 2, "what was already built still counts");
        assert!(
            document.get("builds").is_some(),
            "the rest of the registry document is untouched: {document}"
        );
    }
}
