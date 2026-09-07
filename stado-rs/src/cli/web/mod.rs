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

// `Declare` carries every flag the three kinds of declaration between them
// need, so it is much larger than `List` or `Quality`. Boxing a clap
// subcommand variant would put an indirection in the parser's own type for
// nothing: this enum is constructed once per process. `ReleaseCommands`
// carries the same allow for the same reason.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub(crate) enum WebCommands {
    /// Declare a web product: where it runs, as whom, and on which hostname.
    ///
    /// With `--redirect-to` the product is a hostname and a target and
    /// nothing else: the edge answers it with a 308 and there is no unit, no
    /// release and no host, so `--host`, `--port` and `--consumer` are
    /// refused beside it.
    Declare {
        /// Product name, matching `product` in its `.wisent-release.json`.
        name: String,
        /// Registry target the unit runs on.
        #[arg(
            long,
            required_unless_present_any = ["redirect_to", "upstream_service"],
            conflicts_with_all = ["redirect_to", "upstream_service"]
        )]
        host: Option<String>,
        /// Loopback port the unit listens on.
        #[arg(
            long,
            required_unless_present_any = ["redirect_to", "upstream_service"],
            conflicts_with_all = ["redirect_to", "upstream_service"]
        )]
        port: Option<u16>,
        /// Public hostname the product answers on.
        #[arg(long)]
        hostname: String,
        /// Skarbiec consumer identity the unit runs as.
        #[arg(
            long,
            required_unless_present_any = ["redirect_to", "upstream_service"],
            conflicts_with_all = ["redirect_to", "upstream_service"]
        )]
        consumer: Option<String>,
        /// Where this hostname redirects, instead of running a unit:
        /// an https URL with a host and no query or fragment.
        #[arg(long = "redirect-to", conflicts_with_all = ["upstream_service", "path_prefix"])]
        redirect_to: Option<String>,
        /// A path prefix under a hostname another declaration owns, for a unit
        /// product mounted inside that hostname's site block.
        #[arg(long = "path-prefix", conflicts_with_all = ["redirect_to", "upstream_service"])]
        path_prefix: Option<String>,
        /// A registry service this hostname is published in front of, instead
        /// of a unit this product owns. The service directory answers which
        /// host it is active on and which address it serves.
        #[arg(long = "upstream-service")]
        upstream_service: Option<String>,
        /// Request path that proves the unit is ready.
        #[arg(long, default_value = "/", conflicts_with_all = ["redirect_to", "upstream_service"])]
        readyz: String,
        /// Which edge terminates TLS for the hostname.
        #[arg(
            long,
            default_value = "stado",
            value_parser = clap::builder::PossibleValuesParser::new(crate::config::WEB_API_EDGES)
        )]
        edge: String,
        /// Plain environment entry, `NAME=value`; repeatable.
        #[arg(long = "env", conflicts_with_all = ["redirect_to", "upstream_service"])]
        env: Vec<String>,
        /// Secret environment entry, `NAME=item#field`; repeatable.
        #[arg(long = "secret", conflicts_with_all = ["redirect_to", "upstream_service"])]
        secrets: Vec<String>,
        /// Declared database the product reads.
        #[arg(long, conflicts_with_all = ["redirect_to", "upstream_service"])]
        database: Option<String>,
        /// Field of the database's Skarbiec item to deliver.
        #[arg(long, default_value = "pooler_url", conflicts_with_all = ["redirect_to", "upstream_service"])]
        database_field: String,
        /// Variable the database field is delivered as.
        #[arg(long, default_value = "DATABASE_URL", conflicts_with_all = ["redirect_to", "upstream_service"])]
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
    /// Report each product's declared edge, live unit/port state, and observed DNS.
    ///
    /// A missing or invalid selected Stado edge is reported in edge_error,
    /// never accepted as an external edge with unknown addresses. Any
    /// non-serving product makes the command exit with status 1.
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
    Quality {
        /// The directory the site is served from, repository-relative.
        ///
        /// Only a static site names one. Absent, the site root is the
        /// checkout root.
        #[arg(long)]
        root: Option<String>,
    },
    /// Build the checked-out web product and stage its runnable tarball.
    ///
    /// Runs on a release worker, inside the checkout Stado prepared.
    Build {
        /// The directory the site is served from, repository-relative.
        ///
        /// Only a static site names one — for a product whose build script
        /// writes into `dist/`, or whose site is not at the repository root.
        /// Absent, the site root is the checkout root.
        #[arg(long)]
        root: Option<String>,
    },
}

pub(crate) async fn dispatch(command: WebCommands) -> Result<(), CmdError> {
    match command {
        WebCommands::Declare {
            name,
            host,
            port,
            hostname,
            consumer,
            redirect_to,
            upstream_service,
            path_prefix,
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
            // clap guarantees the three unit arguments are present unless
            // this is a redirect, so the defaults here are only ever reached
            // by a redirect, which has no unit to describe.
            host: host.as_deref().unwrap_or_default(),
            port: port.unwrap_or_default(),
            hostname: &hostname,
            consumer: consumer.as_deref().unwrap_or_default(),
            redirect_to: redirect_to.as_deref(),
            upstream_service: upstream_service.as_deref(),
            path_prefix: path_prefix.as_deref(),
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
        WebCommands::Quality { root } => build::quality(root.as_deref()),
        WebCommands::Build { root } => build::build(root.as_deref()),
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
    redirect_to: Option<&'a str>,
    upstream_service: Option<&'a str>,
    path_prefix: Option<&'a str>,
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
    // One variable, one source. `--env NAME=value` writes into the unit's own
    // environment and `--secret NAME=item#field` writes into the env file the
    // launcher sources afterwards, so declaring both leaves the value decided
    // by the order two different writers happen to run in, and the
    // declaration says two things about one name. On 2026-09-06 the
    // Preferences declaration carried `--env NEXT_PUBLIC_BASE_URL=https://...`
    // and `--secret NEXT_PUBLIC_BASE_URL=NEXT_PUBLIC_BASE_URL#value` together
    // and this command accepted it without a word. The database variable is
    // the third writer of the same file and is checked against both.
    for name in secrets.keys() {
        if env.contains_key(name) {
            return Err(CmdError::usage(format!(
                "{name:?} is declared as both --env and --secret; one variable has one source. \
                 Drop the --secret for a value that is public, or the --env for one that is not"
            )));
        }
    }
    if request.database.is_some() {
        let variable = request.database_variable;
        if env.contains_key(variable) || secrets.contains_key(variable) {
            return Err(CmdError::usage(format!(
                "--database-variable {variable:?} is also declared as --env or --secret; the \
                 database plane and that declaration would write the same variable"
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
    entry.insert("hostname".into(), json!(request.hostname));
    entry.insert("edge".into(), json!(request.edge));
    // A redirect declares a hostname, a target and the edge that answers it.
    // Writing a host, a port, a consumer and a readiness path beside it would
    // put a unit in the configuration that nothing ever installs, and the
    // parser refuses that combination anyway.
    if let Some(target) = request.redirect_to {
        if !crate::config::is_redirect_target(target) {
            return Err(CmdError::usage(format!(
                "--redirect-to {target:?} must be an https URL with a host, no query or fragment, \
                 and no trailing slash"
            )));
        }
        entry.insert("redirect_to".into(), json!(target));
    } else if let Some(service) = request.upstream_service {
        // The service is checked against the directory now rather than at the
        // first `route`: a hostname declared in front of a service nobody
        // declared is a declaration that cannot be rendered, and finding that
        // out here costs one read instead of a failed publication.
        entry.insert("upstream_service".into(), json!(service));
    } else {
        entry.insert("host".into(), json!(request.host));
        entry.insert("port".into(), json!(request.port));
        entry.insert("consumer".into(), json!(request.consumer));
        // A mount answers at its own prefix, so the owner's readiness path is
        // not its readiness path and `route` proves it at `<prefix>/` instead.
        if let Some(prefix) = request.path_prefix {
            if !crate::config::is_mount_prefix(prefix) {
                return Err(CmdError::usage(format!(
                    "--path-prefix {prefix:?} must be an absolute path with no trailing slash, like \"/docs\""
                )));
            }
            // The owner is required now rather than at the first `route`: a
            // mount is rendered inside its owner's site block, so one with no
            // owner is a block with nowhere to go, and the hostname would get
            // no certificate at all.
            let declared_products = crate::config::web_api_products().ok();
            let owner = declared_products
                .into_iter()
                .flatten()
                .find(|(other, product)| {
                    other.as_str() != request.name
                        && product.hostname() == request.hostname
                        && product.owns_its_hostname()
                })
                .map(|(other, _)| other.clone());
            let Some(owner) = owner else {
                return Err(CmdError::usage(format!(
                    "no declaration owns {}, so {prefix} has no site block to be mounted in: declare the product that answers that hostname first, without --path-prefix",
                    request.hostname
                )));
            };
            println!(
                "mounting {prefix} on {}, owned by {owner}",
                request.hostname
            );
            entry.insert("path_prefix".into(), json!(prefix));
        } else {
            entry.insert("readyz".into(), json!(request.readyz));
        }
    }
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
    // Each kind reports what it is. A redirect and an upstream-service
    // hostname have no host, no port and no consumer, and printing an empty
    // host on port 0 as an empty consumer described a unit that does not
    // exist.
    let mut report = json!({
        "product": request.name,
        "hostname": request.hostname,
        "edge": request.edge,
        "change": if existed.get() { "replaced" } else { "declared" },
    });
    let object = report
        .as_object_mut()
        .expect("a JSON object was just built");
    let summary = if let Some(target) = request.redirect_to {
        object.insert("kind".into(), json!("redirect"));
        object.insert("redirect_to".into(), json!(target));
        format!("redirect to {target}")
    } else if let Some(service) = request.upstream_service {
        object.insert("kind".into(), json!("upstream-service"));
        object.insert("upstream_service".into(), json!(service));
        format!("in front of service {service}")
    } else {
        object.insert("kind".into(), json!("unit"));
        object.insert("host".into(), json!(request.host));
        object.insert("port".into(), json!(request.port));
        object.insert("consumer".into(), json!(request.consumer));
        object.insert("unit".into(), json!(unit_label(request.name)));
        format!(
            "on {}:{} as {}",
            request.host, request.port, request.consumer
        )
    };
    if request.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} {} {summary} -> https://{}",
            report["change"].as_str().unwrap_or_default(),
            request.name,
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
            // A hostname-only product has no host, port, consumer or unit,
            // and printing empties for them described a unit that does not
            // exist. Each kind carries the fields it actually has.
            let mut row = json!({
                "product": name,
                "hostname": product.hostname(),
                "edge": product.edge(),
            });
            let object = row.as_object_mut().expect("a JSON object was just built");
            match (product.redirect_to(), product.upstream_service()) {
                (Some(target), _) => {
                    object.insert("kind".into(), json!("redirect"));
                    object.insert("redirect_to".into(), json!(target));
                }
                (None, Some(service)) => {
                    object.insert("kind".into(), json!("upstream-service"));
                    object.insert("upstream_service".into(), json!(service));
                }
                (None, None) => {
                    object.insert("kind".into(), json!("unit"));
                    object.insert("host".into(), json!(product.host()));
                    object.insert("port".into(), json!(product.port()));
                    object.insert("consumer".into(), json!(product.consumer()));
                    object.insert("unit".into(), json!(unit_label(name)));
                    object.insert("readyz".into(), json!(product.readyz()));
                    object.insert(
                        "database".into(),
                        json!(product.database().map(|database| json!({
                            "name": database.name(),
                            "field": database.field(),
                            "variable": database.variable(),
                        }))),
                    );
                    object.insert(
                        "secrets".into(),
                        json!(product.secrets().keys().cloned().collect::<Vec<_>>()),
                    );
                }
            }
            row
        })
        .collect();
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            let product = row["product"].as_str().unwrap_or_default();
            let hostname = row["hostname"].as_str().unwrap_or_default();
            let edge = row["edge"].as_str().unwrap_or_default();
            match row["kind"].as_str().unwrap_or_default() {
                "redirect" => println!(
                    "{product} redirect hostname={hostname} to={} edge={edge}",
                    row["redirect_to"].as_str().unwrap_or_default()
                ),
                "upstream-service" => println!(
                    "{product} upstream-service hostname={hostname} service={} edge={edge}",
                    row["upstream_service"].as_str().unwrap_or_default()
                ),
                _ => println!(
                    "{product} unit host={} port={} hostname={hostname} consumer={} edge={edge} unit={}",
                    row["host"].as_str().unwrap_or_default(),
                    row["port"].as_u64().unwrap_or_default(),
                    row["consumer"].as_str().unwrap_or_default(),
                    row["unit"].as_str().unwrap_or_default(),
                ),
            }
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
