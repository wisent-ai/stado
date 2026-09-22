//! Validate declarations before writing or using them; unknown is not zero.
use crate::fleet_expansion::constants::*;
use crate::fleet_expansion::model::Catalog;
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;

pub(crate) fn money(value: f64, field: &str) -> Result<i64, String> {
    if !value.is_finite() || !(0.0..=MAX_MONEY_USD).contains(&value) {
        return Err(format!(
            "{field} must be finite and between 0 and {MAX_MONEY_USD} USD"
        ));
    }
    let cents = value * CENTS_PER_USD;
    if (cents - cents.round()).abs() > CENT_PRECISION_TOLERANCE {
        return Err(format!("{field} must have at most two decimal places"));
    }
    Ok(cents.round() as i64)
}

pub(crate) fn timestamp(raw: &str, field: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(raw)
        .map(|v| v.with_timezone(&Utc))
        .map_err(|error| format!("{field} must be an RFC3339 timestamp: {error}"))
}

pub(crate) fn identifier(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= MAX_IDENTIFIER_BYTES
        && raw
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_')
}

pub(crate) fn catalog(catalog: &Catalog) -> Result<(), String> {
    if catalog.schema_version != SCHEMA_VERSION {
        return Err("unsupported expansion catalog schema_version".into());
    }
    if catalog.options.len() > MAX_OPTIONS {
        return Err(format!(
            "expansion catalog allows at most {MAX_OPTIONS} options; no candidates were discarded"
        ));
    }
    let mut ids = BTreeSet::new();
    for option in &catalog.options {
        if !identifier(&option.id) || !ids.insert(&option.id) {
            return Err(format!(
                "expansion option id must be a unique lowercase identifier: {}",
                option.id
            ));
        }
        for (field, value) in [
            ("label", &option.label),
            ("benefit_group", &option.benefit_group),
            ("evidence", &option.evidence),
        ] {
            if value.trim().is_empty() || value.len() > MAX_TEXT_BYTES {
                return Err(format!(
                    "option {} {field} must contain 1..{MAX_TEXT_BYTES} bytes",
                    option.id
                ));
            }
        }
        let mut keys = BTreeSet::new();
        if option.need_keys.is_empty() {
            return Err(format!("option {} needs at least one need_key", option.id));
        }
        for key in &option.need_keys {
            let valid = key.split_once(':').is_some_and(|(kind, subject)| {
                matches!(kind, "ram" | "storage" | "gpu" | "cpu" | "host")
                    && !subject.trim().is_empty()
                    && subject.len() <= MAX_SUBJECT_BYTES
            });
            if !valid || !keys.insert(key) {
                return Err(format!(
                    "option {} has invalid or duplicate need_key: {key}",
                    option.id
                ));
            }
        }
        for (field, value) in [
            ("upfront_usd", option.upfront_usd),
            ("monthly_cost_usd", option.monthly_cost_usd),
            ("monthly_savings_usd", option.monthly_savings_usd),
            ("monthly_margin_usd", option.monthly_margin_usd),
        ] {
            if let Some(value) = value {
                money(value, &format!("option {} {field}", option.id))?;
            }
        }
        if option.lead_time_days > MAX_LEAD_TIME_DAYS {
            return Err(format!(
                "option {} lead_time_days exceeds {MAX_LEAD_TIME_DAYS}",
                option.id
            ));
        }
        let observed = timestamp(&option.observed_at, "observed_at")?;
        let expires = timestamp(&option.valid_until, "valid_until")?;
        if expires <= observed {
            return Err(format!(
                "option {} valid_until must follow observed_at",
                option.id
            ));
        }
    }
    Ok(())
}
