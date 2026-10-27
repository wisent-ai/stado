//! `stado database rest place NAME --port PORT --key-valid-days DAYS`: the
//! HTTP API a hosted provider served in front of a Postgres database, served
//! by the fleet in front of a fleet database, so a consumer built on the
//! provider's client library moves with its database.
//!
//! PostgREST is the server: it answers the provider's REST dialect
//! (`/rest/v1/<table>` with `select`, filters and `Prefer` headers) and
//! switches to the Postgres role a request's key names. The placement makes
//! what it needs on the database: the roles `anon`, `authenticated` and
//! `service_role` (`NOLOGIN`; a restored provider dump already carries them
//! and its policies name them) and `authenticator` (`LOGIN`, with a password
//! generated here, a member of the three). It writes
//! `~/.stado/databases/<name>/postgrest.conf` (owner-only: it holds the
//! authenticator password and the signing secret), installs the managed unit
//! `<name>-rest` that runs `postgrest` on it, and writes onto the database's
//! credential item what a consumer reads: `rest_url`, `jwt_secret`,
//! `anon_key` and `service_role_key`, signed with the secret for the days
//! the caller states. A placement already made is reported, never made twice;
//! the keys are minted again only with `--rotate-keys`.

mod jwt;
pub(super) mod retire;
mod roles;

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::dump::files;
use super::dump::tools::{program, Connection};
use super::supabase::owner_vault;
use super::DatabaseProvider;
use roles::{generated, ANON, AUTHENTICATOR, SERVICE};

/// What `<name>` gains to name the API's unit; `destroy` retires it too.
pub(in crate::cli::database) const UNIT_SUFFIX: &str = "-rest";
const CONFIG: &str = "postgrest.conf";
/// The schemas the API exposes: the provider exposes `public` by default.
const SCHEMAS: &str = "public";

/// `stado database rest <verb>`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum RestCommands {
    /// Place PostgREST beside a fleet database as the unit `<name>-rest`:
    /// make the roles it switches to, write its configuration, mint the
    /// anon and service_role keys onto `<name>-database` as `rest_url`,
    /// `jwt_secret`, `anon_key` and `service_role_key`.
    Place(RestPlaceArgs),
    /// The inverse of place: stop and forget `<name>-rest` on the
    /// database's host and take the API's fields off `<name>-database`.
    Retire(retire::RestRetireArgs),
}

/// `stado database rest place NAME --port PORT --key-valid-days DAYS`.
#[derive(Debug, clap::Args)]
pub(crate) struct RestPlaceArgs {
    /// Declared fleet postgres database the API serves.
    pub(super) name: String,
    /// Port the API listens on: Stado assumes none.
    #[arg(long)]
    pub(super) port: u16,
    /// Days the minted anon and service_role keys stay valid.
    #[arg(long)]
    pub(super) key_valid_days: u32,
    /// Mint new keys with a new secret even when the API is already placed; every consumer holding the old keys loses access.
    #[arg(long)]
    pub(super) rotate_keys: bool,
    /// Emit machine-readable output.
    #[arg(long)]
    pub(super) json: bool,
}

pub(in crate::cli::database) async fn place(args: RestPlaceArgs) -> Result<(), CmdError> {
    let RestPlaceArgs {
        name,
        port,
        key_valid_days,
        rotate_keys,
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
            "{name} is not a fleet database ({item}.provider is {recorded}); the fleet serves a REST API only in front of a database it runs: \
             move it first with `stado database migrate {name} --to fleet`"
        )));
    }
    let host = fields["host"]
        .as_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{item} names no host, so the API cannot be placed beside the database"
            ))
            .stating(FailureCode::Config)
        })?;
    if host != here {
        // The API is placed beside the database, so the placement runs
        // there, through the host channel, and its one-line report is
        // printed here. NAME is a checked token and the numbers print
        // without spaces, so the line splits back into exactly its words.
        let mut line = format!(
            "database rest place {name} --port {port} --key-valid-days {key_valid_days} --json"
        );
        if rotate_keys {
            line.push_str(" --rotate-keys");
        }
        let arguments: Vec<&str> = line.split_whitespace().collect();
        let output = crate::cli::host::remote_stado_output(host, &arguments)
            .await
            .map_err(|error| {
                let mut wrapped = CmdError::click(format!(
                    "the REST API of {name} was not placed on {host}: {error}"
                ));
                wrapped.failure = error.failure;
                wrapped
            })?;
        let report = super::fleet::last_json(&output).ok_or_else(|| {
            CmdError::click(format!(
                "{name}: stado database rest place on {host} printed no report: {}",
                output.trim()
            ))
            .stating(FailureCode::InfraDown)
        })?;
        return answer(&report, json_output);
    }
    let address = fields["address"]
        .as_str()
        .filter(|address| !address.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{item} names no address, so no consumer could reach the API"
            ))
            .stating(FailureCode::Config)
        })?;
    let unit = format!("{name}{UNIT_SUFFIX}");
    let installed =
        crate::deploy::service::declared_services(&crate::cli::canonical_host(&here).await?)
            .iter()
            .any(|service| service.matches(&unit));
    let keyed = fields
        .get("jwt_secret")
        .and_then(Value::as_str)
        .is_some_and(|secret| !secret.is_empty());
    if installed && keyed && !rotate_keys {
        let report =
            json!({ "reused": true, "unit": unit, "rest_url": fields["rest_url"], "item": item });
        return answer(&report, json_output);
    }

    let connection = Connection::from_item(&item, &fields)?;
    let directory = crate::config_file::home()
        .map_err(|cause| {
            CmdError::click(format!(
                "a fleet database and its API live under the home, and there is none: {cause}"
            ))
            .stating(FailureCode::Config)
        })?
        .join(".stado")
        .join("databases")
        .join(&name);
    let ca_file = files::server_ca(&directory, "rest", &connection)?;
    let ca = ca_file.as_deref();
    let psql = program("psql", None)?;
    let postgrest = program("postgrest", None)?;

    let authenticator_password = roles::prepare(&psql, &connection, ca)?;

    let secret = generated();
    let issued_at = chrono::Utc::now();
    let anon_key = jwt::mint(secret.as_bytes(), ANON, issued_at, key_valid_days)?;
    let service_key = jwt::mint(secret.as_bytes(), SERVICE, issued_at, key_valid_days)?;
    // The database listens on the item's routable address and port, the same
    // the fleet placement wrote into its session URL.
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
    let db_uri = format!(
        "postgres://{AUTHENTICATOR}:{}@{db_host}:{db_port}/postgres?{ssl}",
        super::supabase::userinfo(&authenticator_password)
    );
    let config = directory.join(CONFIG);
    files::write(
        &config,
        &format!(
            "db-uri = \"{db_uri}\"\ndb-schemas = \"{SCHEMAS}\"\ndb-anon-role = \"{ANON}\"\njwt-secret = \"{secret}\"\nserver-port = {port}\n"
        ),
    )?;
    files::restrict(&config)?;

    let rest_url = format!("http://{db_host}:{port}");
    let mut written = fields.clone();
    written["rest_url"] = json!(rest_url);
    written["rest_port"] = json!(port);
    written["jwt_secret"] = json!(secret);
    written["anon_key"] = json!(anon_key);
    written["service_role_key"] = json!(service_key);
    written["keys_issued_at"] = json!(issued_at.to_rfc3339());
    written["keys_valid_days"] = json!(key_valid_days);
    let document = owner.document(&item).await?.ok_or_else(|| {
        CmdError::click(format!("{item} is not in {}'s owner vault", owner.name()))
            .stating(FailureCode::NotFound)
    })?;
    owner
        .store(&item, "bundle", &written, &document["context"])
        .await?;

    if installed {
        // The unit runs on the configuration it started with; the new secret
        // and authenticator password reach it only through a restart.
        crate::cli::service::lifecycle::release::unit::restart_quietly(&unit, Some(&here))
            .await
            .map_err(|error| {
                error.within(format!(
                    "{item} carries the new keys and {} is written, but {unit} still runs on the old ones",
                    config.display()
                ))
            })?;
    } else {
        crate::cli::service::lifecycle::deploy::deploy(crate::cli::service::lifecycle::deploy::DeployOptions {
            name: &unit,
            host: Some(&here),
            host_heuristic: None,
            from: Some(postgrest.display().to_string()),
            from_artifact: None,
            args: &[config.display().to_string()],
            launchd_label: None,
            as_launch_agent: false,
            as_json: true,
        })
        .await
        .map_err(|error| {
            let mut wrapped = CmdError::click(format!(
                "{item} carries the API's keys and {} is written, but the unit {unit} was not installed: {error}",
                config.display()
            ));
            wrapped.failure = error.failure;
            wrapped
        })?;
    }
    let report = json!({
        "reused": false,
        "database": name,
        "unit": unit,
        "config": config.display().to_string(),
        "rest_url": rest_url,
        "item": item,
        "keys_valid_days": key_valid_days,
        "keys_rotated": rotate_keys,
        "consumers": "each reads rest_url and anon_key or service_role_key from the item on its next start",
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
