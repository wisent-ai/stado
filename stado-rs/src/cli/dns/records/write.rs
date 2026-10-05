//! The merge, and the two verbs built on it: author one record, remove one
//! record.
//!
//! Namecheap has no per-record write, so both verbs are the same three steps —
//! read the whole zone, change exactly one name and type, write the whole zone
//! back — and both then read the zone a third time, because a write that the
//! registrar accepted and did not apply must not be reported as success.

use serde_json::{json, Value};

use crate::cli::CmdError;

use super::super::registrar::zone::Zone;
use super::super::registrar::Registrar;
use super::super::WRITABLE_TYPES;
use super::{get_hosts, row, set_hosts, Record};

pub(in crate::cli::dns) fn normalized_type(record_type: &str) -> Result<String, CmdError> {
    let upper = record_type.trim().to_ascii_uppercase();
    if WRITABLE_TYPES.contains(&upper.as_str()) {
        Ok(upper)
    } else {
        Err(CmdError::usage(format!(
            "record type {record_type:?} is not one Stado authors; supported: {}",
            WRITABLE_TYPES.join(", ")
        )))
    }
}

/// Replace one name and type in the zone. Returns the merged list and what
/// the change is, so callers can report it without a second read.
///
/// `ttl` is the operator's when given; without it the record keeps the TTL
/// the zone already states for it, and a new record states none, so the
/// registrar applies its own default. The same holds for MX preference.
pub(in crate::cli::dns) fn merge(
    records: &[Record],
    host: &str,
    record_type: &str,
    address: &str,
    ttl: Option<&str>,
) -> (Vec<Record>, &'static str, Vec<Record>) {
    let replaced: Vec<Record> = records
        .iter()
        .filter(|record| record.host == host && record.record_type == record_type)
        .cloned()
        .collect();
    let kept_ttl = ttl
        .map(str::to_string)
        .or_else(|| (replaced.len() == 1).then(|| replaced[0].ttl.clone()))
        .unwrap_or_default();
    let kept_mx_pref = if replaced.len() == 1 {
        replaced[0].mx_pref.clone()
    } else {
        Default::default()
    };
    let unchanged =
        replaced.len() == 1 && replaced[0].address == address && replaced[0].ttl == kept_ttl;
    let mut merged: Vec<Record> = records
        .iter()
        .filter(|record| !(record.host == host && record.record_type == record_type))
        .cloned()
        .collect();
    merged.push(Record {
        host: host.to_string(),
        record_type: record_type.to_string(),
        address: address.to_string(),
        mx_pref: kept_mx_pref,
        ttl: kept_ttl,
    });
    let change = if unchanged {
        "unchanged"
    } else if replaced.is_empty() {
        "created"
    } else {
        "replaced"
    };
    (merged, change, replaced)
}

/// Write one record into a zone and verify it afterwards.
///
/// Used by `stado dns set` and by `stado web route`, which is the reason it is
/// public: a product's hostname and an operator's hand-typed record must take
/// the same path through the registrar, or the merge has two implementations
/// again.
pub(crate) async fn ensure_record(
    name: &str,
    record_type: &str,
    value: &str,
    ttl: Option<&str>,
    zone: Option<&str>,
    credential: &str,
) -> Result<Value, CmdError> {
    let record_type = normalized_type(record_type)?;
    let zone = match zone {
        Some(zone) => Zone::parse(zone)?,
        None => Zone::of(name)?,
    };
    let host = zone.host_of(name)?;
    let registrar = Registrar::read(credential).await?;
    let before = get_hosts(&registrar, &zone).await?;
    let (merged, change, replaced) = merge(&before, &host, &record_type, value, ttl);
    if change != "unchanged" {
        set_hosts(&registrar, &zone, &merged).await?;
        let after = get_hosts(&registrar, &zone).await?;
        let visible = after.iter().any(|record| {
            record.host == host && record.record_type == record_type && record.address == value
        });
        if !visible {
            return Err(CmdError::click(format!(
                "{name} {record_type} {value} is not visible in zone {} after the write",
                zone.name
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        if after.len() != merged.len() {
            return Err(CmdError::click(format!(
                "zone {} holds {} records after a write of {}; the zone was not merged as sent",
                zone.name,
                after.len(),
                merged.len()
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
    }
    Ok(json!({
        "zone": zone.name,
        "name": name,
        "host": host,
        "type": record_type,
        "value": value,
        "ttl": ttl,
        "change": change,
        "replaced": replaced.iter().map(row).collect::<Vec<_>>(),
        "records_before": before.len(),
        "records_after": merged.len(),
    }))
}

/// Delete one record from a zone and verify it is gone.
///
/// Public for the same reason [`ensure_record`] is: a product's hostname and
/// an operator's hand-typed record must take the same path through the
/// registrar, or the whole-zone merge has two implementations again — and the
/// deleting one is the half that removes records nobody named.
pub(crate) async fn remove_record(
    name: &str,
    record_type: &str,
    zone: Option<&str>,
    credential: &str,
) -> Result<Value, CmdError> {
    let record_type = normalized_type(record_type)?;
    let zone = match zone {
        Some(zone) => Zone::parse(zone)?,
        None => Zone::of(name)?,
    };
    let host = zone.host_of(name)?;
    let registrar = Registrar::read(credential).await?;
    let before = get_hosts(&registrar, &zone).await?;
    let kept: Vec<Record> = before
        .iter()
        .filter(|record| !(record.host == host && record.record_type == record_type))
        .cloned()
        .collect();
    let removed = before.len() - kept.len();
    if removed > 0 {
        set_hosts(&registrar, &zone, &kept).await?;
        let after = get_hosts(&registrar, &zone).await?;
        if after
            .iter()
            .any(|record| record.host == host && record.record_type == record_type)
        {
            return Err(CmdError::click(format!(
                "{name} {record_type} is still in zone {} after the removal",
                zone.name
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
    }
    Ok(json!({
        "zone": zone.name,
        "name": name,
        "type": record_type,
        "removed": removed,
        "records_after": kept.len(),
    }))
}
