//! `stado dns delegate <zone>` — move a zone the registrar serves into
//! Cloudflare, records first and nameservers last — and its inverse,
//! `stado dns undelegate <zone>`, which hands the zone back to the registrar.
//!
//! Stado could not publish a hostname by itself: a home host's router can
//! drop inbound web traffic, and a cloud subscription policy can refuse a
//! public address. A zone Cloudflare serves needs neither, because the tunnel
//! Stado already runs carries the traffic out. So the move is a Stado
//! command, not a portal session.
//!
//! The order is the safety. Every record is read from the registrar, every
//! type is checked to have a Cloudflare equivalent, the zone is created and
//! filled, and only when Cloudflare holds all of them are the registrar's
//! nameservers pointed at Cloudflare's — then read back. A record type
//! Cloudflare cannot hold (Namecheap's URL redirects) refuses the move before
//! anything is written, naming the records, because moving the nameservers
//! would silently drop them.

use serde_json::json;

use crate::cli::cloudflare::{import_zone, zone_entries, ZoneEntry};
use crate::cli::CmdError;

use super::records::{get_hosts, Record};
use super::registrar::zone::Zone;
use super::registrar::{call, Registrar};

/// Namecheap types Cloudflare holds as the same type, and ALIAS, which
/// Cloudflare serves at the apex as a flattened CNAME.
const CARRIED: &[&str] = &["A", "AAAA", "CNAME", "MX", "TXT", "NS", "ALIAS"];

/// The Cloudflare TTL for "automatic".
const AUTOMATIC_TTL: u32 = 1;

fn entry(zone: &Zone, record: &Record) -> Result<ZoneEntry, String> {
    let record_type = record.record_type.to_ascii_uppercase();
    if !CARRIED.contains(&record_type.as_str()) {
        return Err(format!(
            "{} {} {}",
            record.record_type, record.host, record.address
        ));
    }
    let name = if record.host == "@" {
        zone.name.clone()
    } else {
        format!("{}.{}", record.host, zone.name)
    };
    let priority = (record_type == "MX")
        .then(|| record.mx_pref.trim().parse::<u16>().ok())
        .flatten();
    Ok(ZoneEntry {
        name,
        record_type: if record_type == "ALIAS" {
            "CNAME".to_string()
        } else {
            record_type
        },
        content: record.address.trim_end_matches('.').to_string(),
        priority,
        ttl: record.ttl.trim().parse().unwrap_or(AUTOMATIC_TTL),
    })
}

/// The nameservers the registrar currently has for `zone`.
async fn nameservers(registrar: &Registrar, zone: &Zone) -> Result<Vec<String>, CmdError> {
    let mut parameters = registrar.base(zone);
    parameters.push((
        "Command".into(),
        "namecheap.domains.dns.getList".to_string(),
    ));
    let body = call(parameters).await?;
    let mut found: Vec<String> = regex::Regex::new(r"<Nameserver>([^<]+)</Nameserver>")
        .expect("static regex compiles")
        .captures_iter(&body)
        .map(|capture| capture[1].trim().trim_end_matches('.').to_ascii_lowercase())
        .collect();
    found.sort();
    Ok(found)
}

pub(super) async fn delegate(
    zone: &str,
    api_credential: &str,
    credential: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let zone = Zone::parse(zone)?;
    let registrar = Registrar::read(credential).await?;
    let records = get_hosts(&registrar, &zone).await?;
    let mut entries = Vec::with_capacity(records.len());
    let mut uncarried = Vec::new();
    for record in &records {
        match entry(&zone, record) {
            Ok(entry) => entries.push(entry),
            Err(described) => uncarried.push(described),
        }
    }
    if !uncarried.is_empty() {
        return Err(CmdError::refused(format!(
            "{} was not moved: Cloudflare has no record type for {}; replace them in the zone \
             with records Cloudflare holds, then run this again. Nothing was written.",
            zone.name,
            uncarried.join(", ")
        )));
    }

    let imported = import_zone(api_credential, &zone.name, &entries).await?;
    let mut wanted: Vec<String> = imported
        .name_servers
        .iter()
        .map(|server| server.trim_end_matches('.').to_ascii_lowercase())
        .collect();
    wanted.sort();
    let before = nameservers(&registrar, &zone).await?;
    if before != wanted {
        let mut parameters = registrar.base(&zone);
        parameters.push((
            "Command".into(),
            "namecheap.domains.dns.setCustom".to_string(),
        ));
        parameters.push(("Nameservers".into(), wanted.join(",")));
        call(parameters).await?;
    }
    let after = nameservers(&registrar, &zone).await?;
    if after != wanted {
        return Err(CmdError::click(format!(
            "{} holds all {} records in Cloudflare, but the registrar answers nameservers {} \
             after being set to {}",
            zone.name,
            entries.len(),
            after.join(", "),
            wanted.join(", ")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "zone": zone.name,
                "cloudflare_zone": imported.zone_id,
                "cloudflare_status": imported.status,
                "records": entries.len(),
                "created": imported.created,
                "already_present": imported.already,
                "nameservers_before": before,
                "nameservers": after,
            }))?
        );
    } else {
        for created in &imported.created {
            println!("created {created}");
        }
        println!(
            "{}: {} records in Cloudflare ({} created, {} already there); nameservers {} -> {}; \
             Cloudflare zone status {}",
            zone.name,
            entries.len(),
            imported.created.len(),
            imported.already,
            before.join(", "),
            after.join(", "),
            imported.status
        );
    }
    Ok(())
}

/// Whether two entries name the same record: Cloudflare quotes TXT content
/// and reports names in lower case, so neither difference counts.
fn same_entry(left: &ZoneEntry, right: &ZoneEntry) -> bool {
    left.record_type.eq_ignore_ascii_case(&right.record_type)
        && left.name.eq_ignore_ascii_case(&right.name)
        && left.content.trim_matches('"').trim_end_matches('.')
            == right.content.trim_matches('"').trim_end_matches('.')
        && left.priority == right.priority
}

/// Point the registrar back at its own nameservers, but only when every
/// record Cloudflare serves is also in the registrar's host list: moving the
/// nameservers back would otherwise silently drop the records added since the
/// zone was delegated. The refusal names them and changes nothing.
pub(super) async fn undelegate(
    zone: &str,
    api_credential: &str,
    credential: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let zone = Zone::parse(zone)?;
    let registrar = Registrar::read(credential).await?;
    let held: Vec<ZoneEntry> = get_hosts(&registrar, &zone)
        .await?
        .iter()
        .filter_map(|record| entry(&zone, record).ok())
        .collect();
    let served = zone_entries(api_credential, &zone.name).await?;
    let missing: Vec<String> = served
        .iter()
        .filter(|record| !held.iter().any(|kept| same_entry(kept, record)))
        .map(|record| format!("{} {} {}", record.record_type, record.name, record.content))
        .collect();
    if !missing.is_empty() {
        return Err(CmdError::refused(format!(
            "{} was not handed back: the registrar's host list lacks {} that Cloudflare serves; \
             add them with `stado dns set` or remove them in Cloudflare, then run this again. \
             Nothing was changed.",
            zone.name,
            missing.join(", ")
        )));
    }

    let before = nameservers(&registrar, &zone).await?;
    let mut parameters = registrar.base(&zone);
    parameters.push((
        "Command".into(),
        "namecheap.domains.dns.setDefault".to_string(),
    ));
    call(parameters).await?;
    let mut parameters = registrar.base(&zone);
    parameters.push((
        "Command".into(),
        "namecheap.domains.dns.getList".to_string(),
    ));
    let listed = call(parameters).await?;
    if !listed.contains("IsUsingOurDNS=\"true\"") {
        return Err(CmdError::click(format!(
            "{} was set back to the registrar's nameservers, but the registrar still answers \
             that it does not serve the zone",
            zone.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let after = nameservers(&registrar, &zone).await?;

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "zone": zone.name,
                "records": held.len(),
                "nameservers_before": before,
                "nameservers": after,
            }))?
        );
    } else {
        println!(
            "{}: served by the registrar again with {} records; nameservers {} -> {}",
            zone.name,
            held.len(),
            before.join(", "),
            after.join(", ")
        );
    }
    Ok(())
}
