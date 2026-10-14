//! Reading and printing the figures the billing APIs report.
//!
//! Providers send the same amount three ways — a JSON number, a decimal
//! string, or Google's units-plus-nanos pair — so a section that only
//! understood one of them printed 0.00 next to a real bill.

use serde_json::Value;

/// One part of Google's units-plus-nanos `Money`, where proto3 JSON leaves
/// out a part that is zero; every other figure is read with [`amount`].
pub(super) fn number(value: Option<&Value>) -> f64 {
    value
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str()?.parse::<f64>().ok())
        })
        .unwrap_or_default()
}

/// A figure a billing API reported, in any of the three ways they send one;
/// `None` when the report does not carry it, so it is printed as not
/// reported instead of as 0.00 beside real figures.
pub(super) fn amount(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_str()?.parse::<f64>().ok())
    })
}

/// A dollar figure as the money section prints it, or `not reported`.
pub(super) fn dollars(value: Option<f64>) -> String {
    match value {
        Some(value) => format!("${value:.2}"),
        None => "not reported".to_string(),
    }
}

pub(super) fn money(value: &Value) -> String {
    let units = number(value.get("units"));
    let nanos = number(value.get("nanos"));
    let currency = value
        .get("currencyCode")
        .and_then(Value::as_str)
        .unwrap_or("USD");
    format!("{currency} {:.2}", units + nanos / 1_000_000_000.0)
}
