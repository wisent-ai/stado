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
/// against.
#[derive(Clone, Debug)]
pub struct Credentials {
    pub item: String,
    pub pooler_url: String,
    pub ca_certificate: String,
}

#[derive(Deserialize)]
struct Resolution {
    credential_item: String,
}

#[derive(Deserialize)]
struct Route {
    url: String,
}

/// `stado <arguments>`, with exactly `environment` when one is given, so no
/// ambient variable selects the identity a credential read runs under.
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
    let operation = format!("stado {}", arguments.join(" "));
    let mut command = Command::new(&stado);
    command.args(arguments).stdin(Stdio::null());
    if let Some(environment) = environment {
        command.env_clear();
        for (name, value) in environment {
            command.env(name, value);
        }
    }
    let output = command
        .output()
        .await
        .map_err(|error| Error::new(step, format!("{operation} could not start: {error}")))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(Error::new(
            step,
            format!("{operation} exited {}: {}", output.status, detail.trim()),
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
        ("STADO_CREDENTIALS_ADMIN_URL", route.to_owned()),
        (
            "STADO_CREDENTIALS_ADMIN_CONSUMER",
            database.credential_consumer.clone(),
        ),
        (
            "STADO_CREDENTIALS_ADMIN_TOKEN_FILE",
            database.token_file().display().to_string(),
        ),
    ];
    let arguments = [CREDENTIALS_GROUP, "get", item, "--field", field];
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
/// underscores, letters upper case) and `<PRODUCT>_DATABASE_CA_FILE`, the
/// PEM bundle the server is verified against. Nothing when the URL is unset;
/// a refusal naming the variable when it is set and its certificate is not.
fn from_environment(database: &FleetDatabase) -> Option<Result<Credentials, Error>> {
    let prefix = database.name.to_uppercase().replace('-', "_");
    let url_variable = format!("{prefix}_DATABASE_URL");
    let url = std::env::var(&url_variable)
        .ok()
        .filter(|value| !value.trim().is_empty())?;
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
                pooler_url: url.trim().to_string(),
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
    let ca_certificate = field(database, &route.url, &item, "ca_certificate").await?;
    Ok(Credentials {
        item,
        pooler_url,
        ca_certificate,
    })
}
