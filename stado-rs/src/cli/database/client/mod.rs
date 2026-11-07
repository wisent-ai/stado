//! `stado database client <name>`: let the database's library client read
//! what `stado_database::connect` reads.
//!
//! A product opens its database through the `stado-database` crate as the
//! Skarbiec consumer `<name>-database-client`, whose bearer the vault owner
//! keeps in `~/.stado/<name>-database-client-skarbiec-token` and every host
//! running the product holds a copy of. The crate reads `pooler_url` and, for
//! a server, `ca_certificate`; a Postgres database is opened on its
//! `session_url`. A consumer on the hosted provider's client library reads
//! instead where the provider's services answer and a key: `provider`, then
//! `url` for a hosted item, or `rest_url`, `auth_url` and `realtime_url` for
//! an item `stado database migrate` moved onto the fleet, with `anon_key` and
//! `service_role_key`. A field the client may not read answers 403 and the
//! product cannot start, so a consumer whose item moved to the fleet would be
//! refused its new addresses. This widens the client's grant on the vault
//! owner to exactly those fields, keeping its bearer, so no host's copy has
//! to be synced again.
//!
//! Skarbiec refuses a capability for a field its item does not hold, so the
//! item is read on the vault owner first and only the fields it holds are
//! granted. Each field it lacks is reported with what writes it (the REST
//! API's fields once `stado database rest place` ran, sign-in's once
//! `stado database auth place` ran); an item without `pooler_url`, which
//! every engine's client reads to connect, is refused by name beside the
//! vault host that answered, and nothing is granted.

use serde_json::{json, Value};

use super::supabase::owner_vault;
use super::writes::report_mutation;
use super::{CmdError, DatabaseProvider};

/// The field every engine's library client reads to connect: an item
/// without it cannot be opened at all.
const POOLER: &str = "pooler_url";

/// The fields `stado_database` reads to connect for an engine.
fn connection_fields(engine: &str) -> &'static [&'static str] {
    match engine {
        "sqlite" => &["pooler_url"],
        "postgres" => &["pooler_url", "session_url", "ca_certificate"],
        _ => &["pooler_url", "ca_certificate"],
    }
}

/// One field a consumer on the hosted provider's client library reads, and
/// what writes it when the item does not hold it yet; `None` for a field the
/// consumer reads only when it is there, so its absence is no gap.
struct ProviderField {
    field: &'static str,
    written_by: Option<&'static str>,
}

const REST_PLACE: &str = "`stado database rest place {name} --port <port> --key-valid-days <days>`";
const AUTH_PLACE: &str = "`stado database auth place {name} --port <port> --ready-within <seconds>`";
const ADOPT: &str = "`stado database adopt {name}`";

/// What a consumer on the hosted provider's client library reads for a
/// Postgres item, by the provider the item records (`None`: it records none,
/// so it was written by a hosted provider before the field existed): which
/// provider serves it, where that provider's services answer and the keys
/// they take. An external server has none of them; no other engine either.
fn provider_fields(engine: &str, provider: Option<DatabaseProvider>) -> Vec<ProviderField> {
    let field = |field, written_by| ProviderField { field, written_by };
    let addresses = match (engine, provider) {
        ("postgres", Some(DatabaseProvider::Fleet)) => vec![
            field("rest_url", Some(REST_PLACE)),
            field("auth_url", Some(AUTH_PLACE)),
            field("realtime_url", None),
        ],
        ("postgres", Some(DatabaseProvider::Supabase) | None) => vec![field("url", Some(ADOPT))],
        _ => return Vec::new(),
    };
    let keys = [field("anon_key", Some(REST_PLACE)), field("service_role_key", Some(REST_PLACE))];
    let keys = keys.into_iter().map(|key| match provider {
        Some(DatabaseProvider::Fleet) => key,
        _ => ProviderField { written_by: Some(ADOPT), ..key },
    });
    std::iter::once(field("provider", None)).chain(addresses).chain(keys).collect()
}

/// Whether the item's `fields` object holds `field` with a value.
fn holds(fields: &Value, field: &str) -> bool {
    fields.get(field).is_some_and(|value| !value.is_null())
}

pub(super) async fn client(name: &str, json_output: bool) -> Result<(), CmdError> {
    let declared = super::declared_databases()?;
    let Some(database) = declared.get(name) else {
        let names: Vec<&str> = declared.keys().map(String::as_str).collect();
        return Err(CmdError::refused(format!(
            "unknown database {name:?}; declared: {}",
            names.join(", ")
        )));
    };
    let consumer = format!("{name}-database-client");
    let token_file = format!("~/.stado/{consumer}-skarbiec-token");
    let item = database.item();
    let engine = database.engine();
    let owner = owner_vault::locate().await?;
    let document = owner.document(item).await?.ok_or_else(|| {
        CmdError::refused(format!(
            "{item} is not in the owner vault on {}, so {consumer} has nothing to read",
            owner.name()
        ))
    })?;
    let fields = document
        .get("fields")
        .filter(|fields| fields.is_object())
        .ok_or_else(|| {
            CmdError::click(format!("{item} on {} holds no fields object", owner.name()))
        })?;
    if !holds(fields, POOLER) {
        return Err(CmdError::refused(format!(
            "{item} on {} holds no {POOLER}, which {consumer} reads to connect; nothing was granted",
            owner.name()
        )));
    }
    let mut granted: Vec<&str> = Vec::new();
    let mut not_granted: Vec<Value> = Vec::new();
    for field in connection_fields(engine) {
        if holds(fields, field) {
            granted.push(field);
        } else {
            not_granted.push(json!({
                "field": field,
                "why": format!("{item} on {} holds no {field}", owner.name()),
            }));
        }
    }
    let provider = super::dump::migrate::provider(item, fields)?;
    for wanted in provider_fields(engine, provider) {
        if holds(fields, wanted.field) {
            granted.push(wanted.field);
        } else if let Some(writer) = wanted.written_by {
            not_granted.push(json!({
                "field": wanted.field,
                "why": format!(
                    "{item} holds no {} yet; {} writes it, and running this command again grants it",
                    wanted.field,
                    writer.replace("{name}", name)
                ),
            }));
        }
    }
    let mut answered = String::new();
    for field in &granted {
        answered = crate::cli::host::ensure_declared_read(&consumer, item, field, &token_file)
            .await
            .map_err(|error| {
                let said = match error.message.as_deref() {
                    Some(message) => message.to_string(),
                    None => format!("Skarbiec answered {:?} without a message", error.failure),
                };
                let mut refused = CmdError::click(format!(
                    "{consumer} may not read {item}#{field} yet (vault owner {}): {said}",
                    owner.name()
                ));
                refused.failure = error.failure;
                refused
            })?;
    }
    report_mutation(
        json_output,
        json!({
            "database": name,
            "consumer": consumer,
            "item": item,
            "fields": granted,
            "not_granted": not_granted,
            "owner": answered,
            "token_file": token_file,
        }),
    )
}
