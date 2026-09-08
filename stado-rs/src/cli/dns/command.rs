//! The command surface: what `stado dns` accepts, and what each verb prints.
//!
//! Every arm reads the whole zone through the same merge the rest of this
//! plane uses, so the only thing that lives here is the argument declaration
//! and the two output shapes — one line per record for a person, one document
//! for `--json`.

use clap::Subcommand;
use serde_json::json;

use crate::cli::CmdError;

use super::records::write::{ensure_record, merge, normalized_type, remove_record};
use super::records::{get_hosts, row};
use super::registrar::zone::Zone;
use super::registrar::Registrar;
use super::{DEFAULT_CREDENTIAL, DEFAULT_TTL};

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
