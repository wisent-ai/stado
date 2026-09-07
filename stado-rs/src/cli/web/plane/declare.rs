//! `stado web declare` — writing one product's declaration.

use serde_json::{json, Map, Value};

use super::mutate_web;
use crate::cli::web::unit_label;
use crate::cli::CmdError;

pub(crate) struct DeclareRequest<'a> {
    pub name: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub hostname: &'a str,
    pub consumer: &'a str,
    pub redirect_to: Option<&'a str>,
    pub upstream_service: Option<&'a str>,
    pub path_prefix: Option<&'a str>,
    pub readyz: &'a str,
    pub edge: &'a str,
    pub env: &'a [String],
    pub secrets: &'a [String],
    pub database: Option<&'a str>,
    pub database_field: &'a str,
    pub database_variable: &'a str,
    pub json: bool,
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

pub(crate) fn declare(request: DeclareRequest<'_>) -> Result<(), CmdError> {
    let env = pairs(request.env, "env")?;
    let secrets = pairs(request.secrets, "secret")?;
    for (name, reference) in &secrets {
        let reference = reference.as_str().unwrap_or("");
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

    let entry = entry_for(&request, env, secrets)?;
    let name = request.name.to_string();
    let existed = std::cell::Cell::new(false);
    mutate_web("products", |products| {
        existed.set(products.contains_key(&name));
        products.insert(name.clone(), Value::Object(entry));
        Ok(())
    })?;
    report(&request, existed.get())
}

fn entry_for(
    request: &DeclareRequest<'_>,
    env: Map<String, Value>,
    secrets: Map<String, Value>,
) -> Result<Map<String, Value>, CmdError> {
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
            entry.insert("path_prefix".into(), json!(mount_prefix(request, prefix)?));
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
    Ok(entry)
}

fn mount_prefix<'a>(request: &DeclareRequest<'_>, prefix: &'a str) -> Result<&'a str, CmdError> {
    if !crate::config::is_mount_prefix(prefix) {
        return Err(CmdError::usage(format!(
            "--path-prefix {prefix:?} must be an absolute path with no trailing slash, like \"/docs\""
        )));
    }
    // The owner is required now rather than at the first `route`: a mount is
    // rendered inside its owner's site block, so one with no owner is a block
    // with nowhere to go, and the hostname would get no certificate at all.
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
    Ok(prefix)
}

/// Each kind reports what it is. A redirect and an upstream-service hostname
/// have no host, no port and no consumer, and printing an empty host on port 0
/// as an empty consumer described a unit that does not exist.
fn report(request: &DeclareRequest<'_>, existed: bool) -> Result<(), CmdError> {
    let mut report = json!({
        "product": request.name,
        "hostname": request.hostname,
        "edge": request.edge,
        "change": if existed { "replaced" } else { "declared" },
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
            report["change"].as_str().unwrap_or(""),
            request.name,
            request.hostname,
        );
    }
    Ok(())
}
