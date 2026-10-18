//! Reading and printing the figures the billing APIs report.
//!
//! Providers send the same amount three ways — a JSON number, a decimal
//! string, or Google's units-plus-nanos pair — so a section that only
//! understood one of them printed 0.00 next to a real bill. A figure the
//! report does not carry is said as not reported, and one it carries in no
//! shape a number is read from is said as unreadable with the value: neither
//! is printed as a number nobody reported.

use serde_json::Value;

/// One figure as a billing report carries it.
pub(super) enum Figure {
    Absent,
    Amount(f64),
    Unreadable(Value),
}

impl Figure {
    /// The figure with its sign turned, as a credit reported negative is
    /// printed as the amount it gave.
    pub(super) fn credited(self) -> Self {
        match self {
            Self::Amount(amount) => Self::Amount(-amount),
            other => other,
        }
    }
}

/// A figure a billing API reported, in any of the three ways they send one.
pub(super) fn figure(value: Option<&Value>) -> Figure {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Figure::Absent;
    };
    if let Some(amount) = value.as_f64() {
        return Figure::Amount(amount);
    }
    match value.as_str().map(str::parse::<f64>) {
        Some(Ok(amount)) => Figure::Amount(amount),
        _ => Figure::Unreadable(value.clone()),
    }
}

/// A dollar figure as the money section prints it.
pub(super) fn dollars(figure: &Figure) -> String {
    match figure {
        Figure::Amount(value) => format!("${value:.2}"),
        Figure::Absent => "not reported".to_string(),
        Figure::Unreadable(value) => format!("unreadable ({value})"),
    }
}

/// Nanos in one unit of `google.type.Money`, which carries an amount as
/// whole `units` and a `nanos` fraction of one unit:
/// https://github.com/googleapis/googleapis/blob/master/google/type/money.proto
const NANOS_PER_UNIT: f64 = 1e9;

/// The value of a numeric field proto3 JSON leaves out: it omits a field
/// holding its default, which for a number is zero.
/// https://protobuf.dev/programming-guides/json/
const PROTO3_OMITTED_NUMBER: f64 = 0.0;

/// A `google.type.Money` value. An absent `units` or `nanos` is the part
/// proto3 JSON leaves out; a part present in no numeric shape, or a value
/// naming no currency, is said as such.
pub(super) fn money(value: &Value) -> String {
    let part = |name: &str| match figure(value.get(name)) {
        Figure::Absent => Ok(PROTO3_OMITTED_NUMBER),
        Figure::Amount(amount) => Ok(amount),
        Figure::Unreadable(part) => Err(format!("{name} unreadable ({part})")),
    };
    let currency = match value.get("currencyCode").and_then(Value::as_str) {
        Some(currency) => currency.to_string(),
        None => "(currency not reported)".to_string(),
    };
    match (part("units"), part("nanos")) {
        (Ok(units), Ok(nanos)) => format!("{currency} {:.2}", units + nanos / NANOS_PER_UNIT),
        (Err(error), _) | (_, Err(error)) => format!("{currency} {error}: {value}"),
    }
}
