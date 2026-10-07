//! The three reads behind `connect`, each through the installed `stado`, or
//! the product's own environment on a machine without Stado.

use std::path::PathBuf;
use std::process::Stdio;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;

use crate::{Error, FleetDatabase};

/// Where a resolved database is reached: the credential item that holds it,
/// its connection URL, and the certificate authority the server is verified
/// against. A `sqlite://` file has no server, so its certificate is empty.
/// A Postgres database also carries `session_url`, the route on which one
/// client connection keeps one server session; `pooler_url` may be a
/// transaction-mode pooler that hands each transaction another one.
#[derive(Clone, Debug)]
pub struct Credentials {
    pub item: String,
    pub pooler_url: String,
    pub session_url: Option<String>,
    pub ca_certificate: String,
}

/// A URL naming a SQLite file rather than a server to verify.
pub(crate) fn is_file_url(url: &str) -> bool {
    url.starts_with("sqlite:")
}

/// A URL naming a Postgres server.
pub(crate) fn is_postgres_url(url: &str) -> bool {
    matches!(
        url.split_once("://").map(|(scheme, _)| scheme),
        Some("postgres" | "postgresql")
    )
}

#[derive(Deserialize)]
struct Resolution {
    credential_item: String,
}

#[derive(Deserialize)]
struct Route {
    url: String,
}

/// `stado <arguments>` from the home that holds its binary and configuration.
/// A delegated field read receives only `environment`, never ambient identity.
async fn run(
    database: &FleetDatabase,
    step: &'static str,
    arguments: &[&str],
    environment: Option<&[(&str, String)]>,
) -> Result<String, Error> {
    let stado = database.home.join(".stado/bin/stado");
    if !stado.is_file() {
        return Err(Error::new(
            step,
            format!("Stado is not installed at {}", stado.display()),
        ));
    }
    let mut command = Command::new(&stado);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .env("HOME", &database.home);
    if let Some(environment) = environment {
        command.env_clear();
        for (name, value) in environment {
            command.env(name, value);
        }
    }
    let output = command.output().await.map_err(|error| {
        Error::new(
            step,
            format!("stado {} could not start: {error}", arguments.join(" ")),
        )
    })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(Error::new(
            step,
            format!(
                "stado {} exited {}: {}",
                arguments.join(" "),
                output.status,
                detail.trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn answer<T: DeserializeOwned>(
    database: &FleetDatabase,
    step: &'static str,
    arguments: &[&str],
) -> Result<T, Error> {
    let output = run(database, step, arguments, None).await?;
    serde_json::from_str(&output).map_err(|error| {
        Error::new(
            step,
            format!(
                "stado {} answered unreadable JSON: {error}",
                arguments.join(" ")
            ),
        )
    })
}

/// A value answered as a JSON string, as `{"value": …}`, or as text.
fn decoded(output: &str) -> Option<String> {
    let value = match serde_json::from_str::<Value>(output) {
        Ok(Value::String(value)) => value,
        Ok(Value::Object(object)) => object.get("value")?.as_str()?.to_owned(),
        Ok(_) => return None,
        Err(_) => output.to_owned(),
    };
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

/// The `stado` command group that reads credential fields.
const CREDENTIALS_GROUP: &str = "credentials";

async fn field(
    database: &FleetDatabase,
    route: &str,
    item: &str,
    field: &str,
) -> Result<String, Error> {
    let environment = [
        ("HOME", database.home.display().to_string()),
        ("PATH", std::env::var("PATH").unwrap_or_default()),
        ("TMPDIR", std::env::temp_dir().display().to_string()),
    ];
    let grant_file = database.token_file();
    let grant_file = grant_file
        .to_str()
        .ok_or_else(|| Error::new("read credential field", "the grant file path is not UTF-8"))?;
    let arguments = [
        CREDENTIALS_GROUP,
        "get",
        item,
        "--field",
        field,
        "--route",
        route,
        "--consumer",
        &database.credential_consumer,
        "--grant-file",
        grant_file,
    ];
    let output = run(
        database,
        "read credential field",
        &arguments,
        Some(&environment),
    )
    .await?;
    decoded(&output).ok_or_else(|| {
        Error::new(
            "read credential field",
            format!(
                "stado credentials get {item} --field {field} as {} answered an empty value",
                database.credential_consumer
            ),
        )
    })
}

/// The database a product names in its own environment, for a machine
/// without Stado or Skarbiec: `<PRODUCT>_DATABASE_URL` (dashes become
/// underscores, letters upper case) and, for a server, `<PRODUCT>_DATABASE_CA_FILE`,
/// the PEM bundle it is verified against. Nothing when the URL is unset; a
/// refusal naming the variable when a server's certificate is not set.
fn from_environment(database: &FleetDatabase) -> Option<Result<Credentials, Error>> {
    let prefix = database.name.to_uppercase().replace('-', "_");
    let url_variable = format!("{prefix}_DATABASE_URL");
    let url = std::env::var(&url_variable)
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let url = url.trim().to_string();
    if is_file_url(&url) {
        return Some(Ok(Credentials {
            item: url_variable,
            pooler_url: url,
            session_url: None,
            ca_certificate: String::new(),
        }));
    }
    let ca_variable = format!("{prefix}_DATABASE_CA_FILE");
    let Some(ca_file) = std::env::var_os(&ca_variable).filter(|value| !value.is_empty()) else {
        return Some(Err(Error::new(
            "read environment",
            format!("{url_variable} is set but {ca_variable} is not; the server is verified against that certificate"),
        )));
    };
    Some(
        std::fs::read_to_string(&ca_file)
            .map_err(|error| {
                Error::new(
                    "read environment",
                    format!(
                        "{ca_variable}={}: {error}",
                        PathBuf::from(&ca_file).display()
                    ),
                )
            })
            .map(|ca_certificate| Credentials {
                item: url_variable,
                // The one URL the product names is the one it is opened on.
                session_url: is_postgres_url(&url).then(|| url.clone()),
                pooler_url: url,
                ca_certificate,
            }),
    )
}

pub(crate) async fn credentials(database: &FleetDatabase) -> Result<Credentials, Error> {
    if let Some(found) = from_environment(database) {
        return found;
    }
    let resolution: Resolution = answer(
        database,
        "resolve database",
        &[
            "database",
            "resolve",
            &database.name,
            "--consumer",
            &database.directory_consumer,
            "--json",
        ],
    )
    .await?;
    let route: Route = answer(
        database,
        "resolve Skarbiec route",
        &[
            "service",
            "directory",
            "connect",
            "skarbiec",
            "--consumer",
            &database.directory_consumer,
            "--json",
        ],
    )
    .await?;
    let item = resolution.credential_item;
    let pooler_url = field(database, &route.url, &item, "pooler_url").await?;
    let ca_certificate = if is_file_url(&pooler_url) {
        String::new()
    } else {
        field(database, &route.url, &item, "ca_certificate").await?
    };
    let session_url = if is_postgres_url(&pooler_url) {
        let url = field(database, &route.url, &item, "session_url")
            .await
            .map_err(|error| {
                Error::new(
                    error.step,
                    format!(
                        "{}; a Postgres item carries session_url, the route that keeps one server session per connection: `stado database adopt {}` writes it for a Supabase database, `stado database create {}` for a fleet or external one",
                        error.detail, database.name, database.name
                    ),
                )
            })?;
        Some(url)
    } else {
        None
    };
    Ok(Credentials {
        item,
        pooler_url,
        session_url,
        ca_certificate,
    })
}
