//! Reading the day out of the registry: when the count resets, and what the
//! recipes themselves say was already submitted.

use serde_json::Value;

use super::DAY_FORMAT;

/// The day after `day`, for the sentence that says when the count resets.
/// A day string the registry holds that does not parse is reported as it
/// is rather than guessed at: a wrong reset time reads as a product fault.
/// The last representable date has no successor and is reported the same
/// way.
pub(super) fn next_day(day: &str) -> String {
    match chrono::NaiveDate::parse_from_str(day, DAY_FORMAT) {
        Ok(date) => match date.succ_opt() {
            Some(next) => next.format(DAY_FORMAT).to_string(),
            None => day.to_string(),
        },
        Err(_not_a_day_in_the_registry_format) => day.to_string(),
    }
}

/// Build jobs the recipes themselves say were submitted on `day`: one per
/// platform run whose recorded instant falls on it and which actually got a
/// job. A run the registry no longer holds — a removed recipe, a platform
/// rebuilt since — is not counted, so this is a floor, never an inflation.
pub(super) fn runs_submitted_on(document: &Value, day: &str) -> u64 {
    // A registry without a `builds` array holds no recipes and so no runs.
    let recipes = match document.get("builds").and_then(Value::as_array) {
        Some(recipes) => recipes.as_slice(),
        None => &[],
    };
    recipes
        .iter()
        .filter_map(|recipe| recipe.get("runs").and_then(Value::as_object))
        .flat_map(|runs| runs.values())
        .filter(|run| {
            run.get("job_id")
                .and_then(Value::as_str)
                .is_some_and(|job| !job.is_empty())
                && run
                    .get("at")
                    .and_then(Value::as_str)
                    .is_some_and(|at| at.starts_with(day))
        })
        .count() as u64
}
