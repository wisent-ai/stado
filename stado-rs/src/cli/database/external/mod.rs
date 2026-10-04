//! `stado database create --provider external`: a database server of any
//! engine the user already runs anywhere -- a managed service such as RDS,
//! Cloud SQL, Neon, PlanetScale, Atlas or Azure, or a server of their own --
//! brought under Stado without Stado creating it. The engine is the scheme
//! of its connection URL: postgres, mysql, mongodb, redis, mssql or any other.
//!
//! The connection URL is read from standard input, never from an argument,
//! so it does not reach the process table or shell history. The server's
//! certificate authority comes from `--ca-certificate`, because consumers
//! open every fleet database with full certificate verification. Both are
//! written into the credential item `<name>-database` in the owner vault,
//! with the same `pooler_url` and `ca_certificate` fields a fleet or Supabase
//! database carries, so a consumer resolves it the same way.

use std::io::Read;
use std::path::Path;

use serde_json::json;

use super::supabase::owner_vault;
use crate::cli::CmdError;
use crate::config::DatabaseEngine;

const URL_SHAPE: &str = "<engine>://user:password@host:port/database";

/// The engine a connection URL's scheme names: the scheme itself, without
/// the `+srv`-style transport suffix, and `postgres` for `postgresql`.
fn engine_of(scheme: &str) -> String {
    let base = scheme.split('+').next().unwrap_or(scheme);
    if base == "postgresql" {
        "postgres".to_string()
    } else {
        base.to_string()
    }
}

/// Create the external database `name`. Its engine is the one its connection
/// URL names; `engine`, when given, must agree with it.
pub(super) async fn create(
    name: &str,
    engine: Option<&str>,
    ca_certificate: &Path,
    consumers: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    if let Some(engine) = engine.filter(|engine| !DatabaseEngine::is_engine_name(engine)) {
        return Err(CmdError::usage(format!(
            "engine {engine:?} is not an engine name: the lowercase scheme of its connection URL, such as postgres, mysql or mongodb"
        )));
    }
    let mut url = String::new();
    std::io::stdin().read_to_string(&mut url).map_err(|error| {
        CmdError::click(format!(
            "reading the connection URL from standard input: {error}"
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let url = url.trim();
    if url.is_empty() {
        return Err(CmdError::usage(format!(
            "--provider external reads the connection URL ({URL_SHAPE}) from standard input, and standard input was empty"
        )));
    }
    let parsed = url::Url::parse(url).map_err(|error| {
        CmdError::usage(format!(
            "the connection URL on standard input is not a URL: {error}"
        ))
    })?;
    let Some(host) = parsed.host_str().map(str::to_string) else {
        return Err(CmdError::usage(format!(
            "the connection URL on standard input names no host; it must be {URL_SHAPE}"
        )));
    };
    // The URL names the engine: postgresql:// is postgres, mongodb+srv:// is
    // mongodb. A TLS scheme such as rediss:// also answers to --engine redis.
    let scheme = parsed.scheme();
    let named = engine_of(scheme);
    let engine = match engine {
        None => named,
        Some(given)
            if given == named || scheme.strip_prefix(given).is_some_and(|rest| rest == "s") =>
        {
            given.to_string()
        }
        Some(given) => {
            return Err(CmdError::usage(format!(
                "the connection URL on standard input is {scheme}://…, but --engine is {given}; drop --engine to take the URL's engine, or give a {given}:// URL"
            )))
        }
    };
    let engine = engine.as_str();
    if DatabaseEngine::parse(engine) == Some(DatabaseEngine::Sqlite) {
        return Err(CmdError::usage(
            "--provider external brings a server; a sqlite file is created with --provider fleet",
        ));
    }
    let declaration = super::verbs::prepare_declaration(
        name,
        engine,
        &["read".to_string(), "write".to_string()],
        consumers,
    )?;
    let certificate = std::fs::read_to_string(ca_certificate).map_err(|error| {
        CmdError::click(format!(
            "--ca-certificate {}: {error}",
            ca_certificate.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    if !certificate.contains("-----BEGIN CERTIFICATE-----") {
        return Err(CmdError::usage(format!(
            "--ca-certificate {} holds no PEM certificate; give the server's certificate authority bundle",
            ca_certificate.display()
        )));
    }
    let owner = owner_vault::locate().await?;
    let item = format!("{name}-database");
    let fields = json!({
        "engine": engine,
        "provider": "external",
        "host": host,
        "pooler_url": url,
        "ca_certificate": certificate,
    });
    let context = json!({ "engine": engine, "provider": "external", "product": name });
    owner.store(&item, "bundle", &fields, &context).await?;
    let declared = declaration.persist()?;
    let outcome = json!({
        "created": name,
        "provider": "external",
        "engine": engine,
        "host": host,
        "item": item,
        "item_vault": owner.name(),
        "declaration": declared,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        println!("database {name}: existing {engine} at {host}, item {item}; declared");
    }
    Ok(())
}
