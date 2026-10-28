//! The credit amount an Azure balance answer states, wherever its API
//! version puts it.

use serde_json::Value;

/// MCA credit balance lives under balanceSummary/currentBalance/value. Legacy
/// APIs return `amount` or `availableBalance`; object values resolve through
/// their `value` member.
pub(super) fn extract_amount(props: &Value) -> Value {
    if let Some(value) = props.pointer("/balanceSummary/currentBalance/value") {
        return value.clone();
    }
    let Some(map) = props.as_object() else {
        return Value::Null;
    };
    let amount = map
        .get("amount")
        .filter(|value| !value.is_null())
        .or_else(|| map.get("availableBalance"));
    match amount {
        // An amount object Azure sent without `value` states no amount; a
        // map index would panic on it.
        Some(amount @ Value::Object(_)) => amount["value"].clone(),
        Some(value) => value.clone(),
        None => Value::Null,
    }
}
