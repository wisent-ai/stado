//! `stado database auth place NAME --port PORT --ready-within SECONDS
//! [--from-project REF] [--reconfigure]`: the sign-in a hosted Supabase
//! project served at `/auth/v1`, served by the fleet beside a fleet
//! database, so the accounts the database's `auth` schema holds keep signing
//! in after the database left the vendor.
//!
//! The server is Supabase Auth (GoTrue), the one the vendor runs, in the
//! release `stado-rs/data/declarations/supabase-auth.json` pins
//! (`stado_product::supabase_auth`): the users, identities, sessions and
//! refresh tokens a migrated database restored (`stado database migrate`)
//! are its own tables, so a password, an OAuth link or a refresh token issued
//! by the vendor works against it unchanged. The placement makes what it
//! needs on the database: the role `supabase_auth_admin` (`LOGIN`, with a
//! password generated here, owner of the `auth` schema and everything in it,
//! as the vendor makes it, so the server's own migrations run). It writes
//! `~/.stado/databases/<name>/auth.env` (owner-only: it holds that password,
//! the signing secret and the providers' secrets), installs the managed unit
//! `<name>-auth` that runs the server with `--config` on it, and writes onto
//! the database's credential item what a consumer reads: `auth_url` and
//! `auth_port`.
//!
//! Tokens the server issues are signed with the item's `jwt_secret`, the one
//! `stado database rest place` gave PostgREST, so a signed-in user's token
//! opens the REST API the policies were written for; the API must be placed
//! first. The project's own settings (site URL, allowed redirects, providers
//! and their secrets, mail, token lifetime) are read from the management API
//! (`GET /v1/projects/{ref}/config/auth`) of the project the item records,
//! or the one `--from-project` names, and written into the configuration, so
//! sign-in behaves as it did. After the unit starts, `/health` is asked once a
//! second until it answers or `--ready-within` seconds pass, since the server
//! migrates its schema before it listens. A placement already made is
//! reported, never made twice; `--reconfigure` writes the configuration again
//! from the project and restarts the unit.

pub(super) mod retire;

use std::path::Path;

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::dump::files;
use super::dump::tools::{program, query, Connection};
use super::rest::roles::{generated, literal};
use super::supabase::owner_vault;
use super::DatabaseProvider;

/// What `<name>` gains to name the server's unit; `destroy` retires it too.
pub(in crate::cli::database) const UNIT_SUFFIX: &str = "-auth";
const CONFIG: &str = "auth.env";
/// The role the server connects as, as the vendor names it.
const ADMIN: &str = "supabase_auth_admin";
/// The schema the server keeps its tables in.
const SCHEMA: &str = "auth";

/// `stado database auth <verb>`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum AuthCommands {
    /// Place Supabase Auth beside a fleet database as the unit
    /// `<name>-auth`: make the role it connects as, write its configuration
    /// from the hosted project's, and write `auth_url` onto `<name>-database`.
    Place(AuthPlaceArgs),
    /// The inverse of place: stop and forget `<name>-auth` on the database's
    /// host and take `auth_url` and `auth_port` off `<name>-database`.
    Retire(retire::AuthRetireArgs),
}

/// `stado database auth place NAME --port PORT`.
#[derive(Debug, clap::Args)]
pub(crate) struct AuthPlaceArgs {
    /// Declared fleet postgres database whose accounts the server serves.
    pub(super) name: String,
    /// Port the server listens on: Stado assumes none.
    #[arg(long)]
    pub(super) port: u16,
    /// The hosted Supabase project whose sign-in settings the server takes,
    /// when the item does not record it (a database migrated before the
    /// item kept its project ref).
    #[arg(long)]
    pub(super) from_project: Option<String>,
    /// Write the configuration again from the project and restart the unit,
    /// even when the server is already placed.
    #[arg(long)]
    pub(super) reconfigure: bool,
    /// Seconds the server is given to answer `/health` after it starts (it
    /// runs its migrations first): Stado assumes none.
    #[arg(long)]
    pub(super) ready_within: u64,
    /// Emit machine-readable output.
    #[arg(long)]
    pub(super) json: bool,
}

/// The server's settings that the hosted project's configuration names by
/// the same word after `GOTRUE_`, upper-cased: every key of the answer that
/// holds a value is one, except those the placement decides itself.
const DECIDED_HERE: &[&str] = &[
    // The placement's own database, address and signing secret.
    "db_max_pool_size",
    "db_max_pool_size_unit",
    "api_max_request_duration",
];

/// One `KEY="value"` line of an env file, the value quoted with its
/// backslashes, quotes and newlines escaped, as GoTrue's loader reads it.
fn line(key: &str, value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("{key}=\"{escaped}\"\n")
}

/// The project's configuration as `GOTRUE_*` lines: each key with a string,
/// number or boolean value, upper-cased behind `GOTRUE_`; a null is a
/// setting the project never made and is left to the server's default. A
/// value of another shape is refused naming the key: the server reads none.
fn project_lines(config: &Value) -> Result<String, CmdError> {
    let Some(settings) = config.as_object() else {
        return Err(CmdError::click(format!(
            "the project's auth configuration is not an object: {config}"
        ))
        .stating(FailureCode::InfraDown));
    };
    let mut written = String::new();
    for (key, value) in settings {
        if DECIDED_HERE.contains(&key.as_str()) {
            continue;
        }
        let text = match value {
            Value::Null => continue,
            Value::String(text) => text.clone(),
            Value::Bool(flag) => flag.to_string(),
            Value::Number(number) => number.to_string(),
            other => {
                return Err(CmdError::click(format!(
                    "the project's auth setting {key} is {other}, which is no single value the server reads"
                ))
                .stating(FailureCode::InfraDown))
            }
        };
        written.push_str(&line(&format!("GOTRUE_{}", key.to_uppercase()), &text));
    }
    Ok(written)
}

/// Make the role the server connects as, give it a new password and the
/// `auth` schema with everything in it, and answer that password.
fn prepare(psql: &Path, connection: &Connection, ca: Option<&Path>) -> Result<String, CmdError> {
    let password = generated();
    let present = !query(
        psql,
        connection,
        ca,
        &format!("select rolname from pg_roles where rolname = {}", literal(ADMIN)),
        &format!("looking up role {ADMIN}"),
    )?
    .trim()
    .is_empty();
    let verb = if present { "alter" } else { "create" };
    query(
        psql,
        connection,
        ca,
        &format!(
            "{verb} role {ADMIN} login noinherit createrole password {}",
            literal(&password)
        ),
        &format!("setting the role {ADMIN}"),
    )?;
    // A restored dump made the schema and its objects with the restoring
    // user (`pg_restore --no-owner`); the server migrates them as their
    // owner, as on the vendor.
    query(
        psql,
        connection,
        ca,
        &format!(
            "create schema if not exists {SCHEMA}; alter schema {SCHEMA} owner to {ADMIN}; \
             do $$ declare object record; begin \
             for object in select format('%I.%I', schemaname, tablename) as name from pg_tables where schemaname = '{SCHEMA}' \
             loop execute format('alter table %s owner to {ADMIN}', object.name); end loop; \
             for object in select format('%I.%I', sequence_schema, sequence_name) as name from information_schema.sequences where sequence_schema = '{SCHEMA}' \
             loop execute format('alter sequence %s owner to {ADMIN}', object.name); end loop; \
             for object in select p.oid::regprocedure::text as name from pg_proc p join pg_namespace n on n.oid = p.pronamespace where n.nspname = '{SCHEMA}' \
             loop execute format('alter function %s owner to {ADMIN}', object.name); end loop; \
             for object in select format('%I.%I', n.nspname, t.typname) as name from pg_type t join pg_namespace n on n.oid = t.typnamespace where n.nspname = '{SCHEMA}' and t.typtype = 'e' \
             loop execute format('alter type %s owner to {ADMIN}', object.name); end loop; \
             end $$;"
        ),
        &format!("giving the {SCHEMA} schema to {ADMIN}"),
    )?;
    Ok(password)
}

pub(in crate::cli::database) async fn place(args: AuthPlaceArgs) -> Result<(), CmdError> {
    let AuthPlaceArgs {
        name,
        port,
        from_project,
        reconfigure,
        ready_within,
        json: json_output,
    } = args;
    let here = crate::cli::release_catalog::this_host().await?;
    let owner = owner_vault::locate().await?;
    let (item, fields) = super::dump::item_fields(&name, &owner).await?;
    let provider = super::dump::migrate::provider(&item, &fields)?;
    if provider != Some(DatabaseProvider::Fleet) {
        let recorded = match provider {
            Some(provider) => provider.to_string(),
            None => "unset".to_string(),
        };
        return Err(CmdError::usage(format!(
            "{name} is not a fleet database ({item}.provider is {recorded}); the fleet serves sign-in only beside a \
             database it runs: move it first with `stado database migrate {name} --to fleet`"
        )));
    }
    let host = fields["host"]
        .as_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{item} names no host, so the sign-in server cannot be placed beside the database"
            ))
            .stating(FailureCode::Config)
        })?;
    if host != here {
        // Placed beside the database, through the host channel; NAME and the
        // project ref are checked tokens, so the line splits back into its
        // words.
        let mut remote =
            format!("database auth place {name} --port {port} --ready-within {ready_within} --json");
        if let Some(reference) = &from_project {
            remote.push_str(&format!(" --from-project {reference}"));
        }
        if reconfigure {
            remote.push_str(" --reconfigure");
        }
        let arguments: Vec<&str> = remote.split_whitespace().collect();
        let output = crate::cli::host::remote_stado_output(host, &arguments)
            .await
            .map_err(|error| {
                let mut wrapped = CmdError::click(format!(
                    "the sign-in server of {name} was not placed on {host}: {error}"
                ));
                wrapped.failure = error.failure;
                wrapped
            })?;
        let report = super::fleet::last_json(&output).ok_or_else(|| {
            CmdError::click(format!(
                "{name}: stado database auth place on {host} printed no report: {}",
                output.trim()
            ))
            .stating(FailureCode::InfraDown)
        })?;
        return answer(&report, json_output);
    }
    let secret = fields["jwt_secret"]
        .as_str()
        .filter(|secret| !secret.is_empty())
        .ok_or_else(|| {
            CmdError::usage(format!(
                "{item} holds no jwt_secret, so tokens the sign-in server issues would open no REST API: place the \
                 API first with `stado database rest place {name} --port <port> --key-valid-days <days>`"
            ))
        })?;
    let address = fields["address"]
        .as_str()
        .filter(|address| !address.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{item} names no address, so no consumer could reach the sign-in server"
            ))
            .stating(FailureCode::Config)
        })?;
    // `migrate` keeps the item as it stood under `<item>-previous`, where the
    // project's ref stays; the fleet item no longer names it. A vault on
    // another host lists its items, so one it does not hold is not read.
    let previous = format!("{item}-previous");
    let held = match owner.item_ids().await? {
        Some(ids) => ids.contains(&previous),
        None => true,
    };
    let kept_reference = match held {
        false => None,
        true => match owner.document(&previous).await? {
            Some(document) => document["fields"]["project_ref"].as_str().map(str::to_string),
            None => None,
        },
    };
    let reference = match (&from_project, fields["project_ref"].as_str(), kept_reference) {
        (Some(reference), _, _) => reference.clone(),
        (None, Some(recorded), _) if !recorded.is_empty() => recorded.to_string(),
        (None, _, Some(kept)) if !kept.is_empty() => kept,
        (None, _, _) => {
            return Err(CmdError::usage(format!(
                "neither {item} nor {previous} records a Supabase project, so the sign-in settings cannot be read: \
                 name the project the accounts came from with --from-project <ref> (`stado database backups {name}` \
                 shows the project_ref each backup was taken from)"
            )))
        }
    };
    let unit = format!("{name}{UNIT_SUFFIX}");
    let installed =
        crate::deploy::service::declared_services(&crate::cli::canonical_host(&here).await?)
            .iter()
            .any(|service| service.matches(&unit));
    let placed = fields
        .get("auth_url")
        .and_then(Value::as_str)
        .is_some_and(|url| !url.is_empty());
    if installed && placed && !reconfigure {
        let report = json!({ "reused": true, "unit": unit, "auth_url": fields["auth_url"], "item": item });
        return answer(&report, json_output);
    }

    let home = crate::config_file::home().map_err(|cause| {
        CmdError::click(format!(
            "a fleet database and its sign-in server live under the home, and there is none: {cause}"
        ))
        .stating(FailureCode::Config)
    })?;
    let (server, version) = stado_product::supabase_auth::ensure(&home).map_err(|error| {
        CmdError::click(format!("the sign-in server of {name} cannot be installed: {error:#}"))
            .stating(FailureCode::InfraDown)
    })?;
    let settings = super::supabase::auth_config(&reference).await.map_err(|error| {
        error.within(format!(
            "the sign-in settings of project {reference} are read so {name}'s accounts sign in as they did"
        ))
    })?;
    let from_project_lines = project_lines(&settings)?;

    let connection = Connection::from_item(&item, &fields)?;
    let directory = home.join(".stado").join("databases").join(&name);
    let ca_file = files::server_ca(&directory, "auth", &connection)?;
    let ca = ca_file.as_deref();
    let psql = program("psql", None)?;
    let admin_password = prepare(&psql, &connection, ca)?;

    let db_host = if address.contains(':') {
        format!("[{address}]")
    } else {
        address.to_string()
    };
    let db_port = fields["port"].as_u64().ok_or_else(|| {
        CmdError::click(format!("{item} names no port")).stating(FailureCode::Config)
    })?;
    let ssl = match &ca_file {
        Some(file) => format!("sslmode=verify-full&sslrootcert={}", file.display()),
        None => "sslmode=require".to_string(),
    };
    let database_url = format!(
        "postgres://{ADMIN}:{}@{db_host}:{db_port}/postgres?{ssl}",
        super::supabase::userinfo(&admin_password)
    );
    let auth_url = format!("http://{db_host}:{port}");
    let server_directory = server.parent().ok_or_else(|| {
        CmdError::click(format!("{} names no directory", server.display())).stating(FailureCode::Config)
    })?;
    // The project's settings first, then what this placement decides, which
    // the loader takes last and so stands.
    let mut config_text = from_project_lines;
    for (key, value) in [
        ("GOTRUE_DB_DRIVER", "postgres".to_string()),
        ("DATABASE_URL", database_url),
        ("GOTRUE_DB_NAMESPACE", SCHEMA.to_string()),
        (
            "GOTRUE_DB_MIGRATIONS_PATH",
            server_directory.join("migrations").display().to_string(),
        ),
        ("GOTRUE_API_HOST", address.to_string()),
        ("PORT", port.to_string()),
        ("API_EXTERNAL_URL", auth_url.clone()),
        ("GOTRUE_JWT_SECRET", secret.to_string()),
        ("GOTRUE_JWT_AUD", "authenticated".to_string()),
        ("GOTRUE_JWT_DEFAULT_GROUP_NAME", "authenticated".to_string()),
        ("GOTRUE_JWT_ADMIN_ROLES", "service_role".to_string()),
    ] {
        config_text.push_str(&line(key, &value));
    }
    let config = directory.join(CONFIG);
    files::write(&config, &config_text)?;
    files::restrict(&config)?;

    let mut written = fields.clone();
    written["auth_url"] = json!(auth_url);
    written["auth_port"] = json!(port);
    written["auth_version"] = json!(version);
    written["auth_settings_from"] = json!(reference);
    let document = owner.document(&item).await?.ok_or_else(|| {
        CmdError::click(format!("{item} is not in {}'s owner vault", owner.name()))
            .stating(FailureCode::NotFound)
    })?;
    owner
        .store(&item, "bundle", &written, &document["context"])
        .await?;

    if installed {
        // The unit runs on the configuration it started with; the new one
        // reaches it only through a restart.
        crate::cli::service::lifecycle::release::unit::restart_quietly(&unit, Some(&here))
            .await
            .map_err(|error| {
                error.within(format!(
                    "{item} carries auth_url and {} is written, but {unit} still runs on the old configuration",
                    config.display()
                ))
            })?;
    } else {
        crate::cli::service::lifecycle::deploy::deploy(crate::cli::service::lifecycle::deploy::DeployOptions {
            name: &unit,
            host: Some(&here),
            host_heuristic: None,
            from: Some(server.display().to_string()),
            from_artifact: None,
            args: &["--config".to_string(), config.display().to_string()],
            launchd_label: None,
            as_launch_agent: false,
            as_json: true,
        })
        .await
        .map_err(|error| {
            let mut wrapped = CmdError::click(format!(
                "{item} carries auth_url and {} is written, but the unit {unit} was not installed: {error}",
                config.display()
            ));
            wrapped.failure = error.failure;
            wrapped
        })?;
    }
    // The server runs its migrations before it listens, so it answers some
    // time after the unit starts: asked again each second until it answers
    // or the time the caller stated passes.
    let health = format!("{auth_url}/health");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(ready_within);
    let mut last = String::new();
    let answered = loop {
        match crate::wait::request(reqwest::Client::new().get(&health)).await {
            Ok(response) if response.status().is_success() => break true,
            Ok(response) => last = format!("answered {}", response.status()),
            Err(error) => last = format!("does not answer: {error}"),
        }
        if std::time::Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    };
    if !answered {
        return Err(CmdError::click(format!(
            "{unit} is installed but {health} {last} after {ready_within}s; its log under ~/.stado/logs names why, \
             and `stado database auth place {name} --port {port} --reconfigure --ready-within <seconds>` asks again"
        ))
        .stating(FailureCode::InfraDown));
    }
    let report = json!({
        "reused": false,
        "database": name,
        "unit": unit,
        "server": server.display().to_string(),
        "version": version,
        "config": config.display().to_string(),
        "settings_from_project": reference,
        "auth_url": auth_url,
        "item": item,
        "consumers": "each reads auth_url (and rest_url, anon_key) from the item on its next start; tokens it issues are \
                      signed with the REST API's jwt_secret",
    });
    answer(&report, json_output)
}

fn answer(report: &Value, json_output: bool) -> Result<(), CmdError> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    crate::cli::print_answer(report, false)
}
