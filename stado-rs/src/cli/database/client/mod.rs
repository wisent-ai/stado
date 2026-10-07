//! `stado database client <name>`: let the database's library client read
//! what `stado_database::connect` reads.
//!
//! A product opens its database through the `stado-database` crate as the
//! Skarbiec consumer `<name>-database-client`, whose bearer the vault owner
//! keeps in `~/.stado/<name>-database-client-skarbiec-token` and every host
//! running the product holds a copy of. The crate reads `pooler_url` and, for
//! a server, `ca_certificate`; a Postgres database is opened on its
//! `session_url`. A field the client may not read answers 403 and the product
//! cannot start. This widens the client's grant on the vault owner to exactly
//! those fields, keeping its bearer, so no host's copy has to be synced again.

use serde_json::json;

use super::writes::report_mutation;
use super::CmdError;

/// The fields `stado_database` reads for an engine.
fn client_fields(engine: &str) -> &'static [&'static str] {
    match engine {
        "sqlite" => &["pooler_url"],
        "postgres" => &["pooler_url", "session_url", "ca_certificate"],
        _ => &["pooler_url", "ca_certificate"],
    }
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
    let fields = client_fields(database.engine());
    let mut owner = String::new();
    for field in fields {
        owner = crate::cli::host::ensure_declared_read(&consumer, item, field, &token_file)
            .await
            .map_err(|error| {
                let mut refused = CmdError::click(format!(
                    "{consumer} may not read {item}#{field} yet: {}",
                    error.message.as_deref().unwrap_or("no detail")
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
            "fields": fields,
            "owner": owner,
            "token_file": token_file,
        }),
    )
}
