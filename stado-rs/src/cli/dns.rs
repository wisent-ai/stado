//! `stado dns` — the records of a zone Stado manages at its registrar.
//!
//! Namecheap has no per-record write. `namecheap.domains.dns.setHosts`
//! replaces the whole host list, so changing one name means re-sending every
//! other record in the zone, and a record left out of that call is deleted.
//! That is why `wisent.com`'s records were written by a script living inside a
//! product repository: the merge had to happen somewhere, and there was
//! nowhere in Stado for it.
//!
//! This is that place. Every command reads the whole zone, merges exactly one
//! name, and writes the whole zone back, so the merge is one implementation
//! the whole fleet shares.
//!
//! Two guards make a whole-zone rewrite safe to run. The parse is counted: if
//! the number of `<host` elements in the response does not equal the number of
//! records read out of it, the command refuses rather than writing a zone that
//! is missing whatever it failed to understand. And `EmailType=MX` travels
//! with every write, because a `setHosts` call without it can reset the mail
//! configuration of a zone that carries custom MX records — this zone carries
//! Google Workspace's.

use clap::Subcommand;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::LazyLock;

use super::CmdError;

const API: &str = "https://api.namecheap.com/xml.response";
const DEFAULT_CREDENTIAL: &str = "namecheap_auto";
const DEFAULT_TTL: &str = "1800";
const DEFAULT_MX_PREF: &str = "10";

/// The record types this plane writes. A zone carries more kinds than these,
/// and every one of them survives a merge untouched; the list bounds what a
/// Stado command will author, not what the zone may hold.
const WRITABLE_TYPES: &[&str] = &["A", "AAAA", "CNAME", "TXT", "ALIAS"];

#[derive(Debug, Subcommand)]
pub(crate) enum DnsCommands {
    /// Print every record in one zone, as the registrar holds it.
    List {
        /// Zone name, for example wisent.com.
        zone: String,
        /// Skarbiec item holding api_user, api_key, username and client_ip.
        #[arg(long, default_value = DEFAULT_CREDENTIAL)]
        credential: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Add or replace one record, preserving every other record in the zone.
    ///
    /// The zone is read, the one name and type are replaced, and the whole
    /// list is written back. The write is verified by reading the zone again;
    /// a record that is not visible afterwards is a failure, not a warning.
    Set {
        /// Fully qualified name, for example preferences.wisent.com.
        name: String,
        /// Record type: A, AAAA, CNAME, TXT or ALIAS.
        #[arg(long = "type", value_name = "TYPE")]
        record_type: String,
        /// Record value: an address for A/AAAA, a target for CNAME/ALIAS.
        #[arg(long)]
        value: String,
        /// Record TTL in seconds.
        #[arg(long, default_value = DEFAULT_TTL)]
        ttl: String,
        /// Zone name; defaults to the last two labels of the name.
        #[arg(long)]
        zone: Option<String>,
        /// Report what would change and exit non-zero, without writing.
        #[arg(long)]
        check: bool,
        /// Skarbiec item holding api_user, api_key, username and client_ip.
        #[arg(long, default_value = DEFAULT_CREDENTIAL)]
        credential: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Remove one record, preserving every other record in the zone.
    Remove {
        /// Fully qualified name to remove.
        name: String,
        /// Record type to remove.
        #[arg(long = "type", value_name = "TYPE")]
        record_type: String,
        /// Zone name; defaults to the last two labels of the name.
        #[arg(long)]
        zone: Option<String>,
        /// Skarbiec item holding api_user, api_key, username and client_ip.
        #[arg(long, default_value = DEFAULT_CREDENTIAL)]
        credential: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) async fn dispatch(command: DnsCommands) -> Result<(), CmdError> {
    match command {
        DnsCommands::List {
            zone,
            credential,
            json,
        } => list(&zone, &credential, json).await,
        DnsCommands::Set {
            name,
            record_type,
            value,
            ttl,
            zone,
            check,
            credential,
            json,
        } => {
            set(
                &name,
                &record_type,
                &value,
                &ttl,
                zone.as_deref(),
                check,
                &credential,
                json,
            )
            .await
        }
        DnsCommands::Remove {
            name,
            record_type,
            zone,
            credential,
            json,
        } => remove(&name, &record_type, zone.as_deref(), &credential, json).await,
    }
}

/// One record of a zone, in the five fields `setHosts` round-trips.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub host: String,
    pub record_type: String,
    pub address: String,
    pub mx_pref: String,
    pub ttl: String,
}

/// The registrar credential: four named fields, read one at a time so the
/// broker never hands over a whole item.
struct Registrar {
    api_user: String,
    api_key: String,
    username: String,
    client_ip: String,
}

impl Registrar {
    async fn read(item: &str) -> Result<Self, CmdError> {
        Ok(Self {
            api_user: field(item, "api_user").await?,
            api_key: field(item, "api_key").await?,
            username: field(item, "username").await?,
            client_ip: field(item, "client_ip").await?,
        })
    }

    fn base(&self, zone: &Zone) -> Vec<(String, String)> {
        vec![
            ("ApiUser".into(), self.api_user.clone()),
            ("ApiKey".into(), self.api_key.clone()),
            ("UserName".into(), self.username.clone()),
            ("ClientIp".into(), self.client_ip.clone()),
            ("SLD".into(), zone.sld.clone()),
            ("TLD".into(), zone.tld.clone()),
        ]
    }
}

async fn field(item: &str, name: &str) -> Result<String, CmdError> {
    crate::credential_store::read_string(item, name)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "credential field {name:?} of {item:?} is required; \
                 the registrar credential carries api_user, api_key, username and client_ip"
            ))
        })
}

/// A zone split the way Namecheap addresses it.
struct Zone {
    name: String,
    sld: String,
    tld: String,
}

impl Zone {
    fn parse(zone: &str) -> Result<Self, CmdError> {
        let zone = zone.trim().trim_end_matches('.').to_ascii_lowercase();
        let Some((sld, tld)) = zone.rsplit_once('.') else {
            return Err(CmdError::usage(format!(
                "{zone:?} is not a zone name; a zone is at least two labels, for example wisent.com"
            )));
        };
        if sld.contains('.') {
            return Err(CmdError::usage(format!(
                "{zone:?} names more than one zone level; \
                 Namecheap addresses a zone as one second-level and one top-level label"
            )));
        }
        if sld.is_empty() || tld.is_empty() {
            return Err(CmdError::usage(format!("{zone:?} is not a zone name")));
        }
        Ok(Self {
            name: zone.clone(),
            sld: sld.to_string(),
            tld: tld.to_string(),
        })
    }

    /// The zone a fully qualified name belongs to, when the caller does not
    /// name one: the last two labels.
    fn of(name: &str) -> Result<Self, CmdError> {
        let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
        let labels: Vec<&str> = name.split('.').collect();
        if labels.len() < 2 {
            return Err(CmdError::usage(format!(
                "{name:?} is not a fully qualified name"
            )));
        }
        Zone::parse(&labels[labels.len() - 2..].join("."))
    }

    /// The host label `setHosts` wants for one fully qualified name: `@` for
    /// the apex, otherwise everything to the left of the zone.
    fn host_of(&self, name: &str) -> Result<String, CmdError> {
        let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
        if name == self.name {
            return Ok("@".to_string());
        }
        name.strip_suffix(&format!(".{}", self.name))
            .map(str::to_string)
            .ok_or_else(|| {
                CmdError::usage(format!(
                    "{name:?} is not inside zone {:?}; name the zone with --zone",
                    self.name
                ))
            })
    }
}

static HOST_ELEMENT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?s)<host\b[^>]*/?>").expect("static regex compiles"));
static ATTRIBUTE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"([A-Za-z]+)="([^"]*)""#).expect("static regex compiles"));
static ERROR_ELEMENT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?s)<Error[^>]*>(.*?)</Error>").expect("static regex compiles")
});

/// XML attribute text, with the five entities an attribute value can carry.
fn unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// POST one command and return the response body, refusing a non-OK status
/// with the registrar's own error text.
async fn call(parameters: Vec<(String, String)>) -> Result<String, CmdError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|error| CmdError::click(error.to_string()))?;
    let response = client
        .post(API)
        .form(&parameters)
        .send()
        .await
        .map_err(|error| CmdError::click(format!("Namecheap API is unreachable: {error}")))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !status.is_success() {
        return Err(CmdError::click(format!("Namecheap answered HTTP {status}")));
    }
    if !body.contains(r#"Status="OK""#) {
        let errors: Vec<String> = ERROR_ELEMENT
            .captures_iter(&body)
            .map(|capture| unescape(capture[1].trim()))
            .filter(|text| !text.is_empty())
            .collect();
        return Err(CmdError::click(format!(
            "Namecheap refused the request: {}",
            if errors.is_empty() {
                body.chars().take(400).collect::<String>()
            } else {
                errors.join("; ")
            }
        )));
    }
    Ok(body)
}

/// Every record in one zone.
///
/// The count guard is the whole reason this is a function and not three lines
/// at the call site: a whole-zone write built from a partial read deletes what
/// the read missed, so a response with more `<host` elements than parsed
/// records is a refusal.
async fn get_hosts(registrar: &Registrar, zone: &Zone) -> Result<Vec<Record>, CmdError> {
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

fn row(record: &Record) -> Value {
    json!({
        "host": record.host,
        "type": record.record_type,
        "address": record.address,
        "ttl": record.ttl,
        "mx_pref": record.mx_pref,
    })
}

async fn list(zone: &str, credential: &str, json_output: bool) -> Result<(), CmdError> {
    let zone = Zone::parse(zone)?;
    let registrar = Registrar::read(credential).await?;
    let records = get_hosts(&registrar, &zone).await?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "zone": zone.name,
                "records": records.iter().map(row).collect::<Vec<_>>(),
            }))?
        );
    } else {
        for record in &records {
            println!(
                "{}\t{}\t{}\tTTL={}",
                record.host, record.record_type, record.address, record.ttl
            );
        }
        println!("{} records in {}", records.len(), zone.name);
    }
    Ok(())
}

fn normalized_type(record_type: &str) -> Result<String, CmdError> {
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
fn merge(
    records: &[Record],
    host: &str,
    record_type: &str,
    address: &str,
    ttl: &str,
) -> (Vec<Record>, &'static str, Vec<Record>) {
    let replaced: Vec<Record> = records
        .iter()
        .filter(|record| record.host == host && record.record_type == record_type)
        .cloned()
        .collect();
    let unchanged = replaced.len() == 1 && replaced[0].address == address && replaced[0].ttl == ttl;
    let mut merged: Vec<Record> = records
        .iter()
        .filter(|record| !(record.host == host && record.record_type == record_type))
        .cloned()
        .collect();
    merged.push(Record {
        host: host.to_string(),
        record_type: record_type.to_string(),
        address: address.to_string(),
        mx_pref: DEFAULT_MX_PREF.to_string(),
        ttl: ttl.to_string(),
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
    ttl: &str,
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
            )));
        }
        if after.len() != merged.len() {
            return Err(CmdError::click(format!(
                "zone {} holds {} records after a write of {}; the zone was not merged as sent",
                zone.name,
                after.len(),
                merged.len()
            )));
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

#[allow(clippy::too_many_arguments)]
async fn set(
    name: &str,
    record_type: &str,
    value: &str,
    ttl: &str,
    zone: Option<&str>,
    check: bool,
    credential: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    if check {
        let record_type = normalized_type(record_type)?;
        let zone = match zone {
            Some(zone) => Zone::parse(zone)?,
            None => Zone::of(name)?,
        };
        let host = zone.host_of(name)?;
        let registrar = Registrar::read(credential).await?;
        let before = get_hosts(&registrar, &zone).await?;
        let (_, change, replaced) = merge(&before, &host, &record_type, value, ttl);
        let report = json!({
            "zone": zone.name,
            "name": name,
            "type": record_type,
            "value": value,
            "change": change,
            "replaced": replaced.iter().map(row).collect::<Vec<_>>(),
            "records": before.len(),
        });
        if json_output {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!("{name} {record_type} {value}: {change}");
        }
        return if change == "unchanged" {
            Ok(())
        } else {
            Err(CmdError::silent(1))
        };
    }
    let report = ensure_record(name, record_type, value, ttl, zone, credential).await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{name} {} {value}: {} ({} records in {})",
            report["type"].as_str().unwrap_or_default(),
            report["change"].as_str().unwrap_or_default(),
            report["records_after"].as_u64().unwrap_or_default(),
            report["zone"].as_str().unwrap_or_default(),
        );
    }
    Ok(())
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
            )));
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

async fn remove(
    name: &str,
    record_type: &str,
    zone: Option<&str>,
    credential: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let report = remove_record(name, record_type, zone, credential).await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{name} {}: removed {} record(s); {} remain in {}",
            report["type"].as_str().unwrap_or_default(),
            report["removed"].as_u64().unwrap_or_default(),
            report["records_after"].as_u64().unwrap_or_default(),
            report["zone"].as_str().unwrap_or_default(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(host: &str, record_type: &str, address: &str) -> Record {
        Record {
            host: host.into(),
            record_type: record_type.into(),
            address: address.into(),
            mx_pref: DEFAULT_MX_PREF.into(),
            ttl: DEFAULT_TTL.into(),
        }
    }

    #[test]
    fn zone_of_takes_the_last_two_labels() {
        let zone = Zone::of("app.preferences.wisent.com").expect("zone");
        assert_eq!(zone.name, "wisent.com");
        assert_eq!(zone.sld, "wisent");
        assert_eq!(zone.tld, "com");
        assert_eq!(
            zone.host_of("app.preferences.wisent.com").expect("host"),
            "app.preferences"
        );
        assert_eq!(zone.host_of("wisent.com").expect("host"), "@");
    }

    #[test]
    fn a_name_outside_the_zone_is_refused() {
        let zone = Zone::parse("wisent.com").expect("zone");
        let error = zone.host_of("preferences.wisent.ai").expect_err("refusal");
        assert!(error.to_string().contains("is not inside zone"), "{error}");
    }

    #[test]
    fn merge_replaces_only_the_named_host_and_type() {
        let before = vec![
            record("@", "A", "76.76.21.21"),
            record("@", "MX", "aspmx.l.google.com"),
            record("preferences", "A", "76.76.21.21"),
        ];
        let (merged, change, replaced) = merge(&before, "preferences", "A", "20.1.2.3", "1800");
        assert_eq!(change, "replaced");
        assert_eq!(replaced.len(), 1);
        assert_eq!(merged.len(), before.len());
        assert!(merged
            .iter()
            .any(|entry| entry.host == "@" && entry.record_type == "MX"));
        assert!(merged
            .iter()
            .any(|entry| entry.host == "preferences" && entry.address == "20.1.2.3"));
    }

    #[test]
    fn merge_reports_an_identical_record_as_unchanged() {
        let before = vec![record("preferences", "A", "20.1.2.3")];
        let (_, change, _) = merge(&before, "preferences", "A", "20.1.2.3", DEFAULT_TTL);
        assert_eq!(change, "unchanged");
    }

    #[test]
    fn merge_creates_a_name_the_zone_does_not_carry() {
        let before = vec![record("@", "A", "76.76.21.21")];
        let (merged, change, replaced) = merge(&before, "app", "A", "20.1.2.3", DEFAULT_TTL);
        assert_eq!(change, "created");
        assert!(replaced.is_empty());
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn attribute_values_are_unescaped() {
        assert_eq!(unescape("v=spf1 &quot;a&quot; &amp; b"), "v=spf1 \"a\" & b");
    }

    #[test]
    fn only_authored_record_types_are_accepted() {
        assert_eq!(normalized_type("a").expect("type"), "A");
        assert!(normalized_type("NS").is_err());
    }
}
