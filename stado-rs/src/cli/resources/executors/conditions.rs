//! Condition matching: whether observed state satisfies a plan's conditions,
//! and the sentence a caller prints when it does not.

use serde_json::Value;

use crate::cli::resources::model::Condition;

pub fn conditions_match(conditions: &[Condition], observed: &Value) -> bool {
    conditions.iter().all(|condition| {
        let actual = field(observed, &condition.field);
        match condition.field.as_str() {
            "minimum_age_seconds" => actual
                .and_then(Value::as_f64)
                .zip(condition.expected.as_f64())
                .is_some_and(|(actual, minimum)| actual >= minimum),
            _ => actual == Some(&condition.expected),
        }
    })
}

pub fn explain_mismatch(conditions: &[Condition], observed: &Value) -> String {
    conditions
        .iter()
        .filter_map(|condition| {
            let actual = field(observed, &condition.field);
            let matches = match condition.field.as_str() {
                "minimum_age_seconds" => actual
                    .and_then(Value::as_f64)
                    .zip(condition.expected.as_f64())
                    .is_some_and(|(actual, minimum)| actual >= minimum),
                _ => actual == Some(&condition.expected),
            };
            (!matches).then(|| {
                format!(
                    "{} expected {}, observed {}",
                    condition.field,
                    condition.expected,
                    actual.cloned().unwrap_or(Value::Null)
                )
            })
        })
        .collect::<Vec<String>>()
        .join("; ")
}

fn field<'a>(value: &'a Value, dotted: &str) -> Option<&'a Value> {
    dotted
        .split('.')
        .try_fold(value, |current, part| current.get(part))
}
