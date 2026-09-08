//! The record itself, and the two whole-zone transfers it travels in: the
//! counted read, and the replacing write.
//!
//! Both halves are here because they are one contract. `setHosts` replaces the
//! whole host list, so a write is only ever as complete as the read it was
//! built from, and the count guard that makes the read trustworthy has to sit
//! next to the call that would delete what a partial read missed.

use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::cli::CmdError;

use super::registrar::zone::Zone;
use super::registrar::{call, unescape, Registrar, ATTRIBUTE, HOST_ELEMENT};
use super::{DEFAULT_MX_PREF, DEFAULT_TTL};

pub(in crate::cli::dns) mod write;

/// One record of a zone, in the five fields `setHosts` round-trips.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub host: String,
    pub record_type: String,
    pub address: String,
    pub mx_pref: String,
    pub ttl: String,
}

/// Every record in one zone.
///
/// The count guard is the whole reason this is a function and not three lines
/// at the call site: a whole-zone write built from a partial read deletes what
/// the read missed, so a response with more `<host` elements than parsed
/// records is a refusal.
pub(super) async fn get_hosts(registrar: &Registrar, zone: &Zone) -> Result<Vec<Record>, CmdError> {
    let mut parameters = registrar.base(zone);
    parameters.push((
        "Command".into(),
        "namecheap.domains.dns.getHosts".to_string(),
    ));
    let body = call(parameters).await?;
    let elements: Vec<&str> = HOST_ELEMENT
        .find_iter(&body)
        .map(|found| found.as_str())
        .collect();
    let mut records = Vec::with_capacity(elements.len());
    for element in &elements {
        let attributes: BTreeMap<String, String> = ATTRIBUTE
            .captures_iter(element)
            .map(|capture| (capture[1].to_string(), unescape(&capture[2])))
            .collect();
        let (Some(host), Some(record_type), Some(address)) = (
            attributes.get("Name"),
            attributes.get("Type"),
            attributes.get("Address"),
        ) else {
            continue;
        };
        records.push(Record {
            host: host.clone(),
            record_type: record_type.clone(),
            address: address.clone(),
            mx_pref: attributes
                .get("MXPref")
                .cloned()
                .unwrap_or_else(|| DEFAULT_MX_PREF.to_string()),
            ttl: attributes
                .get("TTL")
                .cloned()
                .unwrap_or_else(|| DEFAULT_TTL.to_string()),
        });
    }
    if records.len() != elements.len() {
        return Err(CmdError::click(format!(
            "zone {} answered {} host elements but only {} could be read; \
             refusing, because a whole-zone write built from a partial read deletes the rest",
            zone.name,
            elements.len(),
            records.len()
        )));
    }
    Ok(records)
}

/// Replace the zone's host list with `records`.
async fn set_hosts(registrar: &Registrar, zone: &Zone, records: &[Record]) -> Result<(), CmdError> {
    if records.is_empty() {
        return Err(CmdError::click(format!(
            "refusing to write an empty host list to zone {}",
            zone.name
        )));
    }
    let mut parameters = registrar.base(zone);
    parameters.push((
        "Command".into(),
        "namecheap.domains.dns.setHosts".to_string(),
    ));
    // Without this a setHosts call can reset the zone's mail configuration,
    // and this zone's MX records are Google Workspace's.
    parameters.push(("EmailType".into(), "MX".to_string()));
    for (index, record) in records.iter().enumerate() {
        let position = index + 1;
        parameters.push((format!("HostName{position}"), record.host.clone()));
        parameters.push((format!("RecordType{position}"), record.record_type.clone()));
        parameters.push((format!("Address{position}"), record.address.clone()));
        parameters.push((format!("MXPref{position}"), record.mx_pref.clone()));
        parameters.push((format!("TTL{position}"), record.ttl.clone()));
    }
    let body = call(parameters).await?;
    if !body.contains(r#"IsSuccess="true""#) {
        return Err(CmdError::click(
            "Namecheap accepted the request but did not confirm the host update",
        ));
    }
    Ok(())
}

pub(super) fn row(record: &Record) -> Value {
    json!({
        "host": record.host,
        "type": record.record_type,
        "address": record.address,
        "ttl": record.ttl,
        "mx_pref": record.mx_pref,
    })
}
