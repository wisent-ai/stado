//! `stado web` — hosting a Node web product on the fleet.
//!
//! One product is one declaration: which release artifact it runs, on which
//! host and port, under which Skarbiec consumer, with which environment, and
//! behind which public hostname. Everything else in this module is that
//! declaration being acted on.
//!
//! The build half runs on a release worker (`stado web quality`,
//! `stado web build`), so a product's `.wisent-release.json` names one Stado
//! command instead of carrying a build script of its own — thirty-four web
//! products do not need thirty-four of those.
//!
//! The run half is the service registry, unchanged: `stado web deploy` renders
//! the declaration into the same `ServiceDeclaration` any other unit uses and
//! installs it with `stado service deploy`, mints the unit's consumer grant
//! with `stado service grant-sync`, and delivers every secret with
//! `stado service secret-sync` — one field of one item into one variable, over
//! the host channel. A database credential is resolved for the unit's own
//! consumer through `stado database resolve`, so a product that is not a
//! declared consumer of a database cannot receive its credential.
//!
//! The publish half is `stado web route`: the hostname is reconciled into the
//! edge proxy's configuration, then its DNS record moves to the edge, then the
//! hostname is polled until it answers over TLS. That order is forced rather
//! than chosen — Let's Encrypt delivers its challenge to whatever the name
//! resolves to, so the certificate cannot exist until after the record moves,
//! and the site block has to exist before it so the first request after the
//! cutover finds a proxy that knows the name.

mod build;
mod deploy;
mod edge;
mod route;
mod status;

use clap::Subcommand;
use serde_json::{json, Map, Value};

use super::CmdError;
use crate::config::WebApiProduct;

/// Every managed web unit is labelled under one domain, so `launchctl list`
/// and `stado service list` both group them without a naming convention
/// anyone has to remember.
pub(crate) const UNIT_DOMAIN: &str = "com.wisent.web";

/// Where a web product's released bytes are installed on its host. The
/// release machinery already owns `$HOME/.stado/services/<name>/current`.
pub(crate) fn unit_label(product: &str) -> String {
    format!("{UNIT_DOMAIN}.{product}")
}

/// The launcher the staged tarball carries, relative to the install root.
pub(crate) const LAUNCHER: &str = "bin/start-web";

#[derive(Debug, Subcommand)]
pub(crate) enum WebCommands {
    /// Declare a web product: where it runs, as whom, and on which hostname.
    Declare {
        /// Product name, matching `product` in its `.wisent-release.json`.
        name: String,
        /// Registry target the unit runs on.
        #[arg(long)]
        host: String,
        /// Loopback port the unit listens on.
        #[arg(long)]
        port: u16,
        /// Public hostname the product answers on.
        #[arg(long)]
        hostname: String,
        /// Skarbiec consumer identity the unit runs as.
        #[arg(long)]
        consumer: String,
        /// Request path that proves the unit is ready.
        #[arg(long, default_value = "/")]
        readyz: String,
        /// Which edge terminates TLS for the hostname.
        #[arg(
            long,
            default_value = "stado",
            value_parser = clap::builder::PossibleValuesParser::new(crate::config::WEB_API_EDGES)
        )]
        edge: String,
        /// Plain environment entry, `NAME=value`; repeatable.
        #[arg(long = "env")]
        env: Vec<String>,
        /// Secret environment entry, `NAME=item#field`; repeatable.
        #[arg(long = "secret")]
        secrets: Vec<String>,
        /// Declared database the product reads.
        #[arg(long)]
        database: Option<String>,
        /// Field of the database's Skarbiec item to deliver.
        #[arg(long, default_value = "pooler_url")]
        database_field: String,
        /// Variable the database field is delivered as.
        #[arg(long, default_value = "DATABASE_URL")]
        database_variable: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// List declared web products with their host, port, hostname and unit.
    List {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Remove a web product: stop and forget its unit, drop its DNS record.
    Remove {
        /// Product name.
        name: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Install the published release as a managed unit and deliver its
    /// environment, then verify that it answers.
    Deploy {
        /// Product name.
        name: String,
        /// Exact published version; defaults to the newest stable release.
        #[arg(long)]
        version: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// One verdict per product: unit state, its own port, and its hostname.
    Status {
        /// Product name; omit for every declared product.
        name: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Publish a declared hostname: edge, then DNS, then wait for the certificate.
    Route {
        /// Product name.
        name: String,
        /// Report what would change and exit non-zero, without writing.
        #[arg(long)]
        check: bool,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// The public edge: the fleet host that holds an address and terminates
    /// TLS for every `stado`-edge hostname.
    #[command(subcommand)]
    Edge(edge::EdgeCommands),
    /// Install the locked dependency tree and run the product's own checks.
    ///
    /// Runs on a release worker, inside the checkout Stado prepared. The
    /// recipe in `.wisent-release.json` names this command; an operator does
    /// not run it by hand.
    Quality,
    /// Build the checked-out web product and stage its runnable tarball.
    ///
    /// Runs on a release worker, inside the checkout Stado prepared.
    Build,
}

pub(crate) async fn dispatch(command: WebCommands) -> Result<(), CmdError> {
    match command {
        WebCommands::Declare {
            name,
            host,
            port,
            hostname,
            consumer,
            readyz,
            edge,
            env,
            secrets,
            database,
            database_field,
            database_variable,
            json,
        } => declare(DeclareRequest {
            name: &name,
            host: &host,
            port,
            hostname: &hostname,
            consumer: &consumer,
            readyz: &readyz,
            edge: &edge,
            env: &env,
            secrets: &secrets,
            database: database.as_deref(),
            database_field: &database_field,
            database_variable: &database_variable,
            json,
        }),
        WebCommands::List { json } => list(json),
        WebCommands::Remove { name, json } => remove(&name, json).await,
        WebCommands::Deploy {
            name,
            version,
            json,
        } => deploy::deploy(&name, version.as_deref(), json).await,
        WebCommands::Status { name, json } => status::status(name.as_deref(), json).await,
        WebCommands::Route { name, check, json } => route::route(&name, check, json).await,
        WebCommands::Edge(command) => edge::dispatch(command).await,
        WebCommands::Quality => build::quality(),
        WebCommands::Build => build::build(),
    }
}

/// One declared product, read through the configuration plane so the parser
/// that validates a declaration is the only thing that interprets one.
pub(crate) fn product(name: &str) -> Result<&'static WebApiProduct, CmdError> {
    let products = crate::config::web_api_products()
        .map_err(|problems| CmdError::click(problems.join("; ")))?;
    products.get(name).ok_or_else(|| {
        CmdError::usage(format!(
            "no web product {name:?} is declared; declared: {}",
            if products.is_empty() {
                "none".to_string()
            } else {
                products.keys().cloned().collect::<Vec<_>>().join(", ")
            }
        ))
    })
}

/// Load the config file, apply one mutation under `web_api`, refuse anything
/// the plane's own parser rejects, and write atomically.
///
/// The same shape as `stado database`'s mutation, deliberately: a second way
/// to write the configuration is a second thing that can write it wrongly.
pub(crate) fn mutate_web<F>(section: &str, mutation: F) -> Result<Value, CmdError>
where
    F: FnOnce(&mut Map<String, Value>) -> Result<(), String>,
{
    let path = crate::config_file::config_path()
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| CmdError::click("no config file exists; run: stado config init"))?;
    let original = std::fs::read_to_string(&path)?;
    let mut document: Value =
        serde_json::from_str(&original).map_err(|error| CmdError::click(error.to_string()))?;
    if !document.is_object() {
        return Err(CmdError::click("config file must contain a JSON object"));
    }
    let web_api = document
        .as_object_mut()
        .expect("checked above")
        .entry("web_api".to_string())
        .or_insert_with(|| json!({}));
    if !web_api.is_object() {
        return Err(CmdError::click("web_api must be an object"));
    }
    let entry = web_api
        .as_object_mut()
        .expect("checked above")
        .entry(section.to_string())
        .or_insert_with(|| json!({}));
    let map = entry
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("web_api.{section} must be an object")))?;
    mutation(map)?;
    // The parsers refuse an empty map, so a removal that empties the plane
    // collapses the section rather than leaving a document nothing validates.
    if map.is_empty() {
        web_api
            .as_object_mut()
            .expect("checked above")
            .remove(section);
    }
    if web_api
        .as_object()
        .is_some_and(|section| section.is_empty())
    {
        document
            .as_object_mut()
            .expect("checked above")
            .remove("web_api");
    }

    let problems = crate::config_file::validate(&document);
    if !problems.is_empty() {
        return Err(CmdError::click(format!(
            "rejected, config unchanged: {}",
            problems.join("; ")
        )));
    }
    let body = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let temporary = std::path::PathBuf::from(format!("{}.web-setting", path.display()));
    std::fs::write(&temporary, body)?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        std::fs::set_permissions(&temporary, metadata.permissions())?;
    }
    std::fs::rename(&temporary, &path)?;
    Ok(document)
}

struct DeclareRequest<'a> {
    name: &'a str,
    host: &'a str,
    port: u16,
    hostname: &'a str,
    consumer: &'a str,
    readyz: &'a str,
    edge: &'a str,
    env: &'a [String],
    secrets: &'a [String],
    database: Option<&'a str>,
    database_field: &'a str,
    database_variable: &'a str,
    json: bool,
}

/// `NAME=value` pairs, refused one at a time so the operator learns which
/// entry is wrong rather than that one of them is.
fn pairs(values: &[String], label: &str) -> Result<Map<String, Value>, CmdError> {
    let mut parsed = Map::new();
    for value in values {
        let Some((name, rest)) = value.split_once('=') else {
            return Err(CmdError::usage(format!(
                "--{label} {value:?} must be NAME=value"
            )));
        };
        if !crate::config::is_env_name(name) {
            return Err(CmdError::usage(format!(
                "--{label} {value:?} does not start with an environment variable name"
            )));
        }
        if parsed.insert(name.to_string(), json!(rest)).is_some() {
            return Err(CmdError::usage(format!(
                "--{label} names {name:?} more than once"
            )));
        }
    }
    Ok(parsed)
}

fn declare(request: DeclareRequest<'_>) -> Result<(), CmdError> {
    let env = pairs(request.env, "env")?;
    let secrets = pairs(request.secrets, "secret")?;
    for (name, reference) in &secrets {
        let reference = reference.as_str().unwrap_or_default();
        if crate::config::parse_secret_reference(reference).is_none() {
            return Err(CmdError::usage(format!(
                "--secret {name}={reference:?} must reference a Skarbiec item as \"item#field\""
            )));
        }
    }
    // A database is resolved for this product's own consumer, and a consumer
    // the declaration does not list is refused by the database plane. Saying
    // so here turns a deploy-time refusal into a declare-time one.
    if let Some(database) = request.database {
        let databases = crate::config::database_api_databases()
            .map_err(|problems| CmdError::click(problems.join("; ")))?;
        let declared = databases.get(database).ok_or_else(|| {
            CmdError::usage(format!(
                "no database {database:?} is declared; declare it with `stado database declare`"
            ))
        })?;
        if !declared.allows_consumer(request.consumer) {
            return Err(CmdError::usage(format!(
                "consumer {:?} is not authorized for database {database:?}; \
                 grant it with `stado database grant {database} --consumer {}`",
                request.consumer, request.consumer
            )));
        }
    }

    let mut entry = Map::new();
    entry.insert("host".into(), json!(request.host));
    entry.insert("port".into(), json!(request.port));
    entry.insert("hostname".into(), json!(request.hostname));
    entry.insert("consumer".into(), json!(request.consumer));
    entry.insert("readyz".into(), json!(request.readyz));
    entry.insert("edge".into(), json!(request.edge));
    if !env.is_empty() {
        entry.insert("env".into(), Value::Object(env));
    }
    if !secrets.is_empty() {
        entry.insert("secrets".into(), Value::Object(secrets));
    }
    if let Some(database) = request.database {
        entry.insert(
            "database".into(),
            json!({
                "name": database,
                "field": request.database_field,
                "variable": request.database_variable,
            }),
        );
    }

    let name = request.name.to_string();
    let existed = std::cell::Cell::new(false);
    mutate_web("products", |products| {
        existed.set(products.contains_key(&name));
        products.insert(name.clone(), Value::Object(entry));
        Ok(())
    })?;
    let report = json!({
        "product": request.name,
        "host": request.host,
        "port": request.port,
        "hostname": request.hostname,
        "consumer": request.consumer,
        "unit": unit_label(request.name),
        "edge": request.edge,
        "change": if existed.get() { "replaced" } else { "declared" },
    });
    if request.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} {} on {}:{} as {} -> https://{}",
            report["change"].as_str().unwrap_or_default(),
            request.name,
            request.host,
            request.port,
            request.consumer,
            request.hostname,
        );
    }
    Ok(())
}

fn list(json_output: bool) -> Result<(), CmdError> {
    let products = match crate::config::web_api_products() {
        Ok(products) => products,
        // An empty plane is not a broken one: the parser refuses an empty map
        // so that a half-written section cannot pass, and "nothing declared"
        // has to read as nothing declared.
        Err(_) if crate::config_file::get("web_api.products").is_none() => {
            if json_output {
                println!("[]");
            } else {
                println!("no web products are declared");
            }
            return Ok(());
        }
        Err(problems) => return Err(CmdError::click(problems.join("; "))),
    };
    let rows: Vec<Value> = products
        .iter()
        .map(|(name, product)| {
            json!({
                "product": name,
                "host": product.host(),
                "port": product.port(),
                "hostname": product.hostname(),
                "consumer": product.consumer(),
                "unit": unit_label(name),
                "edge": product.edge(),
                "readyz": product.readyz(),
                "database": product.database().map(|database| json!({
                    "name": database.name(),
                    "field": database.field(),
                    "variable": database.variable(),
                })),
                "secrets": product.secrets().keys().cloned().collect::<Vec<_>>(),
            })
        })
        .collect();
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            println!(
                "{} host={} port={} hostname={} consumer={} edge={} unit={}",
                row["product"].as_str().unwrap_or_default(),
                row["host"].as_str().unwrap_or_default(),
                row["port"].as_u64().unwrap_or_default(),
                row["hostname"].as_str().unwrap_or_default(),
                row["consumer"].as_str().unwrap_or_default(),
                row["edge"].as_str().unwrap_or_default(),
                row["unit"].as_str().unwrap_or_default(),
            );
        }
    }
    Ok(())
}

async fn remove(name: &str, json_output: bool) -> Result<(), CmdError> {
    let declared = product(name)?.clone();
    // Order matters and it is the reverse of publication: the record goes
    // first, so nothing resolves to a unit that is about to stop.
    let record = route::retract(name, &declared).await?;
    let unit = deploy::retire(name, &declared).await?;
    let product_name = name.to_string();
    mutate_web("products", |products| {
        products.remove(&product_name);
        Ok(())
    })?;
    let report = json!({
        "product": name,
        "record": record,
        "unit": unit,
        "declaration": "removed",
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{name}: record {}, unit {}, declaration removed",
            record["change"].as_str().unwrap_or("unchanged"),
            unit["change"].as_str().unwrap_or("unchanged"),
        );
    }
    Ok(())
}
