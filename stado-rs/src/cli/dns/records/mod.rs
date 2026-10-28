//! The record itself, and the two whole-zone transfers it travels in: the
//! counted read, and the replacing write.
//!
//! Both halves are here because they are one contract. `setHosts` replaces the
//! whole host list, so a write is only ever as complete as the read it was
//! built from, and the count guard that makes the read trustworthy has to sit
//! next to the call that would delete what a partial read missed.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli::CmdError;

use super::registrar::answer;
use super::registrar::zone::Zone;
use super::registrar::{call, Registrar};

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

/// The getHosts result: one `host` element per record, each attribute named
/// as Namecheap writes it.
#[derive(Deserialize)]
struct GetHostsCommand {
    #[serde(rename = "DomainDNSGetHostsResult")]
    result: GetHostsResult,
}

#[derive(Deserialize)]
struct GetHostsResult {
    #[serde(rename = "host")]
    hosts: Option<Vec<Host>>,
}

#[derive(Deserialize)]
struct Host {
    #[serde(rename = "@Name")]
    name: String,
    #[serde(rename = "@Type")]
    record_type: String,
    #[serde(rename = "@Address")]
    address: String,
    #[serde(rename = "@MXPref")]
    mx_pref: Option<String>,
    #[serde(rename = "@TTL")]
    ttl: Option<String>,
}

/// The setHosts result: whether Namecheap applied the host list.
#[derive(Deserialize)]
struct SetHostsCommand {
    #[serde(rename = "DomainDNSSetHostsResult")]
    result: SetHostsResult,
}

#[derive(Deserialize)]
struct SetHostsResult {
    #[serde(rename = "@IsSuccess")]
    is_success: bool,
}

/// What the registrar stated for an attribute it may leave out. One it left
/// out is carried as no text, and the next write leaves it out too, so the
/// registrar applies its own default to it again.
fn stated(attribute: Option<String>) -> String {
    match attribute {
        Some(text) => text,
        None => String::new(),
    }
}

/// Every record in one zone.
///
/// A whole-zone write built from a partial read deletes what the read
/// missed, so the answer is read whole into its documented shape: a host
/// element without its name, type or address refuses the read with the
/// parser's reason, naming the zone, instead of being passed over.
pub(super) async fn get_hosts(registrar: &Registrar, zone: &Zone) -> Result<Vec<Record>, CmdError> {
    let command = "namecheap.domains.dns.getHosts";
    let mut parameters = registrar.base(zone);
    parameters.push(("Command".into(), command.to_string()));
    let body = call(parameters).await?;
    let answered = answer::read::<GetHostsCommand>(&body).map_err(|error| {
        CmdError::click(format!(
            "zone {} cannot be read whole, so nothing is written over it: {error}",
            zone.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    Ok(answer::result(answered, command)?
        .result
        .hosts
        .into_iter()
        .flatten()
        .map(|host| Record {
            host: host.name,
            record_type: host.record_type,
            address: host.address,
            mx_pref: stated(host.mx_pref),
            ttl: stated(host.ttl),
        })
        .collect())
}

/// Replace the zone's host list with `records`.
async fn set_hosts(registrar: &Registrar, zone: &Zone, records: &[Record]) -> Result<(), CmdError> {
    if records.is_empty() {
        return Err(CmdError::refused(format!(
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
        // Namecheap numbers the hosts of a setHosts call from the first one:
        // a record's position is how many records run up to it.
        let position = records[..=index].len();
        parameters.push((format!("HostName{position}"), record.host.clone()));
        parameters.push((format!("RecordType{position}"), record.record_type.clone()));
        parameters.push((format!("Address{position}"), record.address.clone()));
        if !record.mx_pref.is_empty() {
            parameters.push((format!("MXPref{position}"), record.mx_pref.clone()));
        }
        if !record.ttl.is_empty() {
            parameters.push((format!("TTL{position}"), record.ttl.clone()));
        }
    }
    let body = call(parameters).await?;
    let applied = answer::result(
        answer::read::<SetHostsCommand>(&body)?,
        "namecheap.domains.dns.setHosts",
    )?;
    if !applied.result.is_success {
        return Err(CmdError::click(
            "Namecheap accepted the request but did not confirm the host update",
        )
        .stating(crate::primitives::failure::FailureCode::InfraDown));
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
