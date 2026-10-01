//! A whole zone moved into Cloudflare: the zone created in the account (or
//! found, when it already exists), every record the registrar holds written
//! into it, and the nameservers Cloudflare assigned read back.
//!
//! This is the part of `stado dns delegate` that talks to Cloudflare. A zone
//! Cloudflare serves is what lets the `cloudflare` web edge publish a hostname
//! through the tunnel Stado already runs, with no router, no public address
//! and no charge-bearing cloud resource (85b4d4a6).

use reqwest::Method;
use serde_json::{json, Value};

use super::api::{account_access, required_string, result_array};
use crate::cli::CmdError;

/// One record as Cloudflare takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ZoneEntry {
    pub name: String,
    pub record_type: String,
    pub content: String,
    pub priority: Option<u16>,
    pub ttl: u32,
}

/// What the import did.
#[derive(Debug)]
pub(crate) struct ImportedZone {
    pub zone_id: String,
    pub status: String,
    pub name_servers: Vec<String>,
    pub created: Vec<String>,
    pub already: usize,
}

fn same(existing: &Value, entry: &ZoneEntry) -> bool {
    let text = |key: &str| {
        existing
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    text("type").eq_ignore_ascii_case(&entry.record_type)
        && text("name").eq_ignore_ascii_case(&entry.name)
        && text("content").trim_matches('"') == entry.content.trim_matches('"')
        && existing.get("priority").and_then(Value::as_u64) == entry.priority.map(u64::from)
}

/// Create `zone` in the credential's account, or take the one already there,
/// and make it hold every entry. Records Cloudflare already holds are left as
/// they are; nothing is deleted.
pub(crate) async fn import_zone(
    api_credential: &str,
    zone: &str,
    entries: &[ZoneEntry],
) -> Result<ImportedZone, CmdError> {
    let (account_id, client) = account_access(api_credential).await?;
    let found = client
        .get(
            "/zones",
            &[("name", zone), ("account.id", account_id.as_str())],
        )
        .await?;
    let record = match result_array(&found, "Cloudflare zone lookup")?.first() {
        Some(existing) => existing.clone(),
        None => {
            let created = client
                .write(
                    Method::POST,
                    "/zones",
                    &json!({ "name": zone, "account": { "id": account_id }, "type": "full" }),
                )
                .await?;
            created.get("result").cloned().ok_or_else(|| {
                CmdError::click(format!("Cloudflare created {zone} but returned no zone"))
            })?
        }
    };
    let zone_id = required_string(&record, "id")?;
    let status = required_string(&record, "status")?;
    let name_servers: Vec<String> = record
        .get("name_servers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if name_servers.is_empty() {
        return Err(CmdError::click(format!(
            "Cloudflare assigned no nameservers to {zone}, so the registrar cannot be pointed at it"
        )));
    }

    let path = format!("/zones/{zone_id}/dns_records");
    let listed = client.get(&path, &[("per_page", "5000000")]).await?;
    let existing = result_array(&listed, "Cloudflare DNS record list")?.clone();
    let mut created = Vec::new();
    let mut already = 0;
    for entry in entries {
        if existing.iter().any(|record| same(record, entry)) {
            already += 1;
            continue;
        }
        let mut body = json!({
            "type": entry.record_type,
            "name": entry.name,
            "content": entry.content,
            "ttl": entry.ttl,
            "proxied": false,
        });
        if let Some(priority) = entry.priority {
            body["priority"] = json!(priority);
        }
        client
            .write(Method::POST, &path, &body)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "{zone} is in Cloudflare with {} of {} records written, and {} {} {} was \
                 refused: {}; the registrar still points at its own nameservers",
                    created.len() + already,
                    entries.len(),
                    entry.record_type,
                    entry.name,
                    entry.content,
                    error.message.as_deref().unwrap_or("no detail")
                ))
            })?;
        created.push(format!(
            "{} {} {}",
            entry.record_type, entry.name, entry.content
        ));
    }
    Ok(ImportedZone {
        zone_id,
        status,
        name_servers,
        created,
        already,
    })
}

/// Every record Cloudflare serves for `zone`, as entries the registrar's
/// records are compared with. A zone the account does not hold is a refusal:
/// there is nothing to take back from it.
pub(crate) async fn zone_entries(
    api_credential: &str,
    zone: &str,
) -> Result<Vec<ZoneEntry>, CmdError> {
    let (account_id, client) = account_access(api_credential).await?;
    let found = client
        .get(
            "/zones",
            &[("name", zone), ("account.id", account_id.as_str())],
        )
        .await?;
    let Some(record) = result_array(&found, "Cloudflare zone lookup")?
        .first()
        .cloned()
    else {
        return Err(CmdError::click(format!(
            "Cloudflare holds no zone {zone} in this account, so there is nothing to undelegate"
        )));
    };
    let zone_id = required_string(&record, "id")?;
    let listed = client
        .get(
            &format!("/zones/{zone_id}/dns_records"),
            &[("per_page", "5000000")],
        )
        .await?;
    result_array(&listed, "Cloudflare DNS record list")?
        .iter()
        .map(|existing| {
            Ok(ZoneEntry {
                name: required_string(existing, "name")?,
                record_type: required_string(existing, "type")?,
                content: required_string(existing, "content")?,
                priority: existing
                    .get("priority")
                    .and_then(Value::as_u64)
                    .and_then(|priority| u16::try_from(priority).ok()),
                ttl: existing
                    .get("ttl")
                    .and_then(Value::as_u64)
                    .and_then(|ttl| u32::try_from(ttl).ok())
                    .unwrap_or(1),
            })
        })
        .collect()
}
