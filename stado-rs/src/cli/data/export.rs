//! `stado data export --kind hours|capacity|prices --since-days N --out DIR
//! --key-role ROLE [--json]`: the fleet's own measurements as a dataset that
//! names no host, customer or job.
//!
//! Each kind declares the fields it keeps, and how each identifier is
//! transformed, here and nowhere else; a field a record carries that its kind
//! does not declare is refused, naming it, so a field added to a measurement
//! later never leaves the fleet unreviewed. Hosts become pseudonyms: the
//! first 16 hex digits of HMAC-SHA256 over the host's registry name, keyed
//! with the `key` field of the credential-store item playing `--key-role`.
//! The same key gives the same pseudonym in every export, so datasets join;
//! another key gives unrelated ones; and without the key no pseudonym can be
//! tied back to a host. Board ids are pseudonymised the same way, and times
//! are kept to the hour.
//!
//! The output directory gets `<kind>.jsonl` (one record per line) and
//! `manifest.json`: the kind, the window, every field kept and what was done
//! to it, and the count of records.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

/// The field of the export-key item that holds the key.
const KEY_FIELD: &str = "key";

/// What one kept field is, and what the export did to it.
#[derive(Clone, Copy)]
enum Kept {
    /// A measurement, kept as measured.
    Measured,
    /// A host or board identifier, replaced by its keyed pseudonym.
    Pseudonym,
    /// A time, truncated to the hour.
    Hour,
}

impl Kept {
    fn transform(self) -> &'static str {
        match self {
            Self::Measured => "kept as measured",
            Self::Pseudonym => "replaced by HMAC-SHA256(export key, value), first 16 hex digits",
            Self::Hour => "truncated to the hour (UTC)",
        }
    }
}

/// The declared fields of one kind's output record.
struct Schema {
    kind: &'static str,
    fields: &'static [(&'static str, Kept)],
    /// The fields of the source record this kind reads and does not keep,
    /// each named so a reviewer sees it was dropped on purpose.
    dropped: &'static [&'static str],
}

const HOURS: Schema = Schema {
    kind: "hours",
    fields: &[
        ("host", Kept::Pseudonym),
        ("hour", Kept::Hour),
        ("board", Kept::Pseudonym),
        ("samples", Kept::Measured),
        ("busy_samples", Kept::Measured),
        ("watt_seconds", Kept::Measured),
        ("idle_watt_seconds", Kept::Measured),
        ("idle_seconds", Kept::Measured),
        ("seconds", Kept::Measured),
    ],
    // `served` names the deployment, which names a model the operator chose
    // and may sell; `unread` quotes host-local errors with paths and names.
    dropped: &["cpu_samples", "cpu_busy_sum", "served", "unread"],
};

const CAPACITY: Schema = Schema {
    kind: "capacity",
    fields: &[
        ("host", Kept::Pseudonym),
        ("published_at", Kept::Hour),
        ("accepting_jobs", Kept::Measured),
        ("running_jobs", Kept::Measured),
        ("total_cpu_cores", Kept::Measured),
        ("available_cpu_cores", Kept::Measured),
        ("total_vram_gb", Kept::Measured),
        ("free_vram_gb", Kept::Measured),
        ("total_ram_gb", Kept::Measured),
        ("free_ram_gb", Kept::Measured),
    ],
    // Identifiers, holders, secret names, program versions and diagnostics.
    dropped: &[
        "consumer_id",
        "kind",
        "next_by",
        "running_workloads",
        "available_accelerators",
        "reserved",
        "reservations",
        "ci_jobs_running",
        "ci_jobs_unread",
        "placed_work",
        "secret_fields",
        "recipe_keys",
        "diag",
        "stado_version",
    ],
};

const PRICES: Schema = Schema {
    kind: "prices",
    fields: &[
        ("provider", Kept::Measured),
        ("gpu_name", Kept::Measured),
        ("offers", Kept::Measured),
        ("min_usd_per_hour", Kept::Measured),
        ("median_usd_per_hour", Kept::Measured),
        ("max_usd_per_hour", Kept::Measured),
        ("read_at", Kept::Hour),
    ],
    dropped: &["schema", "price", "source", "occupancy", "occupancy_unread"],
};

fn schema(kind: &str) -> Result<&'static Schema, CmdError> {
    [&HOURS, &CAPACITY, &PRICES]
        .into_iter()
        .find(|schema| schema.kind == kind)
        .ok_or_else(|| {
            CmdError::usage(format!(
                "--kind {kind:?} is not exported; the kinds are hours, capacity and prices"
            ))
        })
}

/// The keyed pseudonym of one identifier.
fn pseudonym(key: &ring::hmac::Key, value: &str) -> String {
    let tag = ring::hmac::sign(key, value.as_bytes());
    hex::encode(&tag.as_ref()[..8])
}

/// An RFC 3339 time truncated to its hour, or the refusal naming the field.
fn hour_of(field: &str, raw: &Value) -> Result<String, String> {
    let text = raw
        .as_str()
        .ok_or_else(|| format!("{field} is not a time: {raw}"))?;
    let at = chrono::DateTime::parse_from_rfc3339(text)
        .map_err(|error| format!("{field} {text:?} is not an RFC 3339 time: {error}"))?
        .with_timezone(&chrono::Utc);
    Ok(
        chrono::DurationRound::duration_trunc(at, chrono::Duration::hours(1))
            .map_err(|error| format!("{field} {text:?} cannot be truncated to the hour: {error}"))?
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

/// Every field of `source` must be kept or declared dropped; one that is
/// neither is refused, naming the field and the record it was found in.
fn refuse_undeclared(
    schema: &Schema,
    source: &Map<String, Value>,
    at: &str,
) -> Result<(), CmdError> {
    for field in source.keys() {
        let kept = schema.fields.iter().any(|(name, _)| name == field);
        if !kept && !schema.dropped.contains(&field.as_str()) {
            return Err(CmdError::declaration(format!(
                "{at} carries the field {field:?}, which the {} export declares neither kept nor \
                 dropped; nothing leaves the fleet unreviewed, so declare it in \
                 stado-rs/src/cli/data/export.rs",
                schema.kind
            )));
        }
    }
    Ok(())
}

/// One output record from the declared fields of `source`, with `host` and
/// `board` (when given) pseudonymised and declared times truncated.
fn record(
    schema: &Schema,
    key: &ring::hmac::Key,
    source: &Map<String, Value>,
    host: Option<&str>,
    board: Option<&str>,
    hour: Option<&str>,
) -> Result<Value, CmdError> {
    let mut out = Map::new();
    for (field, kept) in schema.fields {
        let value = match (*field, kept) {
            ("host", _) => host.map(|host| Value::from(pseudonym(key, host))),
            ("board", _) => board.map(|board| Value::from(pseudonym(key, board))),
            ("hour", _) => hour.map(Value::from),
            (_, Kept::Hour) => match source.get(*field) {
                Some(raw) => Some(Value::from(
                    hour_of(field, raw).map_err(CmdError::declaration)?,
                )),
                None => None,
            },
            (_, _) => source.get(*field).cloned(),
        };
        out.insert((*field).to_string(), value.unwrap_or(Value::Null));
    }
    Ok(Value::Object(out))
}

async fn hours(
    schema: &Schema,
    key: &ring::hmac::Key,
    since: &str,
) -> Result<Vec<Value>, CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = crate::targets::load_registry_from_value(&document)
        .map_err(|error| CmdError::declaration(format!("the registry does not load: {error}")))?;
    let store = crate::queue::JobStorage::new().await?;
    let beacons = crate::cli::registry::beacon_bodies(&store).await?;
    let mut rows = Vec::new();
    for target in &registry.targets {
        let slugs = crate::monitor::host_health::beacon_slugs(target, &target.name);
        let Some(hours) = slugs
            .iter()
            .find_map(|slug| beacons.get(slug))
            .and_then(|beacon| beacon.get("hours"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (start, hour) in hours {
            if start.as_str() < since {
                continue;
            }
            let at = format!("{}'s hour {start}", target.name);
            let Some(hour_fields) = hour.as_object() else {
                return Err(CmdError::declaration(format!("{at} is not an object")));
            };
            // The hour's own fields: `boards` is read below, board by board.
            let mut top = hour_fields.clone();
            top.remove("boards");
            refuse_undeclared(schema, &top, &at)?;
            let start =
                hour_of("hour", &Value::from(start.as_str())).map_err(CmdError::declaration)?;
            for (uuid, board) in hour["boards"].as_object().into_iter().flatten() {
                let Some(board_fields) = board.as_object() else {
                    return Err(CmdError::declaration(format!(
                        "{at} board {uuid} is not an object"
                    )));
                };
                refuse_undeclared(schema, board_fields, &format!("{at} board {uuid}"))?;
                rows.push(record(
                    schema,
                    key,
                    board_fields,
                    Some(&target.name),
                    Some(uuid),
                    Some(&start),
                )?);
            }
        }
    }
    Ok(rows)
}

async fn capacity(
    schema: &Schema,
    key: &ring::hmac::Key,
    since: &str,
) -> Result<Vec<Value>, CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = crate::targets::load_registry_from_value(&document)
        .map_err(|error| CmdError::declaration(format!("the registry does not load: {error}")))?;
    let store = crate::queue::JobStorage::new().await?;
    let publications = crate::queue::capacity::read_publications(&store)
        .await
        .map_err(CmdError::from)?;
    let mut rows = Vec::new();
    // Each host's publication, pseudonymised by the host's registry name as
    // the hours are, so the two kinds of one export join on the pseudonym.
    // A publication no registry host owns is a cloud consumer or a retired
    // machine, which this fleet's dataset does not describe.
    for target in &registry.targets {
        let Some((consumer_id, publication)) =
            crate::queue::capacity::publication_of(&registry, target, &publications).map_err(
                |error| CmdError::declaration(format!("the registry cannot be searched: {error}")),
            )?
        else {
            continue;
        };
        let Some(payload) = publication.payload.as_object() else {
            return Err(CmdError::declaration(format!(
                "the capacity publication of {consumer_id} is not an object"
            )));
        };
        if publication.stamp.is_none_or(|stamp| {
            stamp
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                .as_str()
                < since
        }) {
            continue;
        }
        refuse_undeclared(
            schema,
            payload,
            &format!("the capacity publication of {consumer_id}"),
        )?;
        rows.push(record(
            schema,
            key,
            payload,
            Some(&target.name),
            None,
            None,
        )?);
    }
    Ok(rows)
}

async fn prices(
    schema: &Schema,
    key: &ring::hmac::Key,
    since: &str,
) -> Result<Vec<Value>, CmdError> {
    let store = crate::queue::JobStorage::new().await?;
    let mut rows = Vec::new();
    for path in store
        .list_paths("market/prices/", crate::queue::Listing::Every)
        .await?
    {
        let Some(text) = store.download_text(&path).await? else {
            continue;
        };
        let reading: Value = serde_json::from_str(&text).map_err(|error| {
            CmdError::declaration(format!("{path} is not a price reading: {error}"))
        })?;
        let Some(top) = reading.as_object() else {
            return Err(CmdError::declaration(format!("{path} is not an object")));
        };
        refuse_undeclared(schema, top, &path)?;
        // The figures are the reading's `price`, beside its provider.
        let Some(price) = reading["price"].as_object() else {
            return Err(CmdError::declaration(format!(
                "{path} carries no price object"
            )));
        };
        let mut flat = price.clone();
        flat.insert("provider".into(), reading["provider"].clone());
        if flat.get("read_at").and_then(Value::as_str).unwrap_or("") < since {
            continue;
        }
        refuse_undeclared(schema, &flat, &format!("{path} price"))?;
        rows.push(record(schema, key, &flat, None, None, None)?);
    }
    Ok(rows)
}

/// The export key, read from the credential store; refused naming the role,
/// the field and why when it does not resolve, since an unkeyed pseudonym
/// would be a plain hash anyone could reverse by hashing host names.
async fn export_key(role: &str) -> Result<ring::hmac::Key, CmdError> {
    let reading = crate::providers::host_credential::read_role_field(role, KEY_FIELD).await;
    let Some(secret) = reading.key.as_deref().filter(|secret| secret.len() >= 32) else {
        let cause = match &reading.key {
            Some(_) => "it is shorter than 32 characters".to_string(),
            None => reading.cause(),
        };
        return Err(CmdError::click(format!(
            "the export key {role}/{KEY_FIELD} cannot be used: {cause}; store a random key of at \
             least 32 characters with {}",
            crate::skarbiec::roles::put_command(
                "<vault owner>",
                role,
                "token",
                KEY_FIELD,
                "<random key>"
            )
        ))
        .stating(FailureCode::Config));
    };
    Ok(ring::hmac::Key::new(
        ring::hmac::HMAC_SHA256,
        secret.as_bytes(),
    ))
}

pub(crate) async fn export(
    kind: &str,
    since_days: u32,
    out: &Path,
    key_role: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let schema = schema(kind)?;
    if out.exists()
        && std::fs::read_dir(out)
            .map_err(|error| {
                CmdError::declaration(format!("{} cannot be read: {error}", out.display()))
            })?
            .next()
            .is_some()
    {
        return Err(CmdError::refused(format!(
            "{} is not empty; an export writes into an empty or new directory, so no older file \
             is mistaken for part of it",
            out.display()
        )));
    }
    let key = export_key(key_role).await?;
    let since = (chrono::Utc::now() - chrono::Duration::days(i64::from(since_days)))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let rows = match schema.kind {
        "hours" => hours(schema, &key, &since).await?,
        "capacity" => capacity(schema, &key, &since).await?,
        _ => prices(schema, &key, &since).await?,
    };
    std::fs::create_dir_all(out).map_err(|error| {
        CmdError::declaration(format!("{} cannot be created: {error}", out.display()))
    })?;
    let mut lines = String::new();
    for row in &rows {
        lines.push_str(&serde_json::to_string(row)?);
        lines.push('\n');
    }
    let data_path = out.join(format!("{}.jsonl", schema.kind));
    std::fs::write(&data_path, lines).map_err(|error| {
        CmdError::declaration(format!(
            "{} cannot be written: {error}",
            data_path.display()
        ))
    })?;
    let fields: BTreeMap<&str, &str> = schema
        .fields
        .iter()
        .map(|(field, kept)| (*field, kept.transform()))
        .collect();
    let manifest = json!({
        "schema": "stado.data-export.v1",
        "kind": schema.kind,
        "since": since,
        "exported_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "records": rows.len(),
        "fields": fields,
        "dropped": schema.dropped,
        "pseudonym_key": "HMAC-SHA256 with the export key; the key is not part of the dataset",
        "file": format!("{}.jsonl", schema.kind),
    });
    let manifest_path = out.join("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?).map_err(|error| {
        CmdError::declaration(format!(
            "{} cannot be written: {error}",
            manifest_path.display()
        ))
    })?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&manifest)?);
    } else {
        println!(
            "exported {} {} record(s) since {since} to {}; manifest {}",
            rows.len(),
            schema.kind,
            data_path.display(),
            manifest_path.display()
        );
    }
    Ok(())
}
