//! `stado database create --provider external`: a Postgres the user already
//! runs anywhere -- a managed service such as RDS, Cloud SQL, Neon or Azure,
//! or a server of their own -- brought under Stado without Stado creating it.
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

const URL_SHAPE: &str = "postgres://user:password@host:port/database";

pub(super) async fn create(
    name: &str,
    engine: &str,
    ca_certificate: &Path,
    consumers: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    if engine != "postgres" {
        return Err(CmdError::usage(format!(
            "--provider external brings an existing postgres server; --engine {engine} is created with --provider fleet"
        )));
    }
    let mut url = String::new();
    std::io::stdin().read_to_string(&mut url).map_err(|error| {
        CmdError::click(format!(
            "reading the connection URL from standard input: {error}"
        ))
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
    if !matches!(parsed.scheme(), "postgres" | "postgresql") {
        return Err(CmdError::usage(format!(
            "the connection URL on standard input is {}://…; it must be {URL_SHAPE}",
            parsed.scheme()
        )));
    }
    let certificate = std::fs::read_to_string(ca_certificate).map_err(|error| {
        CmdError::click(format!(
            "--ca-certificate {}: {error}",
            ca_certificate.display()
        ))
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
        "engine": "postgres",
        "provider": "external",
        "host": host,
        "pooler_url": url,
        "ca_certificate": certificate,
    });
    let context = json!({ "engine": "postgres", "provider": "external", "product": name });
    owner.store(&item, "bundle", &fields, &context).await?;
    let declared = super::verbs::declaration(
        name,
        engine,
        &["read".to_string(), "write".to_string()],
        consumers,
    )?;
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
        println!("database {name}: existing postgres at {host}, item {item}; declared");
    }
    Ok(())
}
