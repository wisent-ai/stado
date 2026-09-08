//! Reading and printing the figures the billing APIs report.
//!
//! Providers send the same amount three ways — a JSON number, a decimal
//! string, or Google's units-plus-nanos pair — so a section that only
//! understood one of them printed 0.00 next to a real bill.

use serde_json::Value;

pub(super) fn number(value: Option<&Value>) -> f64 {
    value
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str()?.parse::<f64>().ok())
        })
        .unwrap_or_default()
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
