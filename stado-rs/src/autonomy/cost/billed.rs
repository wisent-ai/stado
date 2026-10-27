//! What the billing snapshot says the providers have already billed.
//!
//! [`month_spent`] is the spend billed so far this calendar month, the figure
//! the month-end projection starts from; [`credit_balance`] is the Azure
//! credit the burn runs down.

use chrono::{DateTime, Utc};
use serde_json::Value;

/// The spend every enabled billing provider has billed this calendar month,
/// or why it is not known.
///
/// A provider's section carries the billing export's per-month net rows
/// (`monthly`); a month with no row has billed nothing yet. A section that is
/// not `ok`, or one that reports no monthly rows at all (Azure's carries a
/// credit balance, not spend), leaves the month's spend unknown. The reader
/// this replaces looked for fields no section writes and read the month as
/// unspent, so a monthly budget passed with the month already spent.
pub(super) fn month_spent(snapshot: Option<&Value>, now: DateTime<Utc>) -> Result<f64, String> {
    let snapshot = snapshot.ok_or_else(|| {
        format!(
            "no billing snapshot is published at {}",
            crate::monitor::billing::BLOB
        )
    })?;
    let providers = crate::config::billing_providers();
    if providers.is_empty() {
        return Err(
            "no billing provider is enabled, so no spend is billed anywhere Stado reads"
                .to_string(),
        );
    }
    let month = now.format("%Y-%m").to_string();
    let mut billed = Vec::new();
    for provider in providers {
        let section = snapshot
            .get(provider.as_str())
            .ok_or_else(|| format!("the billing snapshot carries no {provider} section"))?;
        if section.get("status").and_then(Value::as_str) != Some("ok") {
            let detail = match section.get("detail") {
                Some(detail) => detail,
                None => section,
            };
            return Err(format!("the {provider} billing section is not ok: {detail}"));
        }
        let rows = section
            .get("monthly")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("the {provider} billing section reports no monthly spend"))?;
        for row in rows
            .iter()
            .filter(|row| row.get("month").and_then(Value::as_str) == Some(month.as_str()))
        {
            billed.push(row.get("net").and_then(Value::as_f64).ok_or_else(|| {
                format!("the {provider} billing row for {month} carries no numeric net: {row}")
            })?);
        }
    }
    Ok(billed.into_iter().sum())
}

/// The Azure credit balance the snapshot reports, when it reports one.
pub(super) fn credit_balance(snapshot: Option<&Value>) -> Option<f64> {
    let snapshot = snapshot?;
    snapshot
        .pointer("/azure/available_balance")
        .or_else(|| snapshot.pointer("/azure/balance"))
        .and_then(Value::as_f64)
}
