//! `stado database rest retire NAME`: the inverse of `rest place`. Stops and
//! forgets the unit `<name>-rest` on the database's host and takes the API's
//! coordinates and keys off `<name>-database`, so a consumer reading the item
//! finds no `rest_url` and is refused by name instead of calling an API that
//! is gone. The roles on the database stay: the data's policies name them.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::super::supabase::owner_vault;
use super::UNIT_SUFFIX;

/// The fields `rest place` writes, taken off the item here.
const PLACED_FIELDS: &[&str] = &[
    "rest_url",
    "rest_port",
    "jwt_secret",
    "anon_key",
    "service_role_key",
    "keys_issued_at",
    "keys_valid_days",
];

/// `stado database rest retire NAME`.
#[derive(Debug, clap::Args)]
pub(crate) struct RestRetireArgs {
    /// Declared fleet postgres database whose API is retired.
    pub(super) name: String,
    /// Emit machine-readable output.
    #[arg(long)]
    pub(super) json: bool,
}

pub(in crate::cli::database) async fn retire(args: RestRetireArgs) -> Result<(), CmdError> {
    let RestRetireArgs {
        name,
        json: json_output,
    } = args;
    let owner = owner_vault::locate().await?;
    let (item, fields) = super::super::dump::item_fields(&name, &owner).await?;
    let placed = PLACED_FIELDS
        .iter()
        .any(|field| fields.get(field).is_some_and(|value| !value.is_null()));
    let host = fields["host"]
        .as_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{item} names no host, so there is nowhere the API could run"
            ))
            .stating(FailureCode::Config)
        })?;
    let unit = format!("{name}{UNIT_SUFFIX}");
    let target = crate::cli::canonical_host(host).await?;
    let declared = crate::deploy::service::declared_services(&target)
        .iter()
        .any(|service| service.matches(&unit));
    if !placed && !declared {
        return Err(CmdError::usage(format!(
            "{name} has no REST API to retire: {item} carries none of its fields and {host} declares no unit {unit}"
        )));
    }
    if declared {
        crate::cli::service::lifecycle::adopt::removal::remove(&unit, host, false)
            .await
            .map_err(|error| {
                error.within(format!(
                    "the unit {unit} on {host} was not removed, so {item} keeps the API's fields"
                ))
            })?;
    }
    if placed {
        let mut written = fields.clone();
        if let Value::Object(map) = &mut written {
            for field in PLACED_FIELDS {
                map.remove(*field);
            }
        }
        let document = owner.document(&item).await?.ok_or_else(|| {
            CmdError::click(format!("{item} is not in {}'s owner vault", owner.name()))
                .stating(FailureCode::NotFound)
        })?;
        owner
            .store(&item, "bundle", &written, &document["context"])
            .await?;
    }
    let report = json!({
        "retired": name,
        "unit": unit,
        "host": host,
        "unit_status": if declared { "removed" } else { "already absent" },
        "item": item,
        "fields_removed": if placed { PLACED_FIELDS.to_vec() } else { Vec::new() },
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    crate::cli::print_answer(&report, false)
}
