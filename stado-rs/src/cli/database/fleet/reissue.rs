//! `stado database tls reissue NAME`: a new server certificate for a fleet
//! Postgres, signed by the authority it was placed with, so every consumer
//! keeps verifying it against the `ca_certificate` its item already carries.
//!
//! A certificate `place` issued before it named the loopback by IP only
//! carried `DNS:localhost`; libpq before 15 then checks an IP-address
//! connection against the DNS names alone and refuses it (`server certificate
//! for "localhost" does not match host name "<address>"`), so `stado database
//! backup` of a PostgreSQL 14 fleet server, which runs that release's client,
//! could not copy it. Reissuing writes the certificate `place` writes now
//! (`fleet::postgres::tls::server_certificate`) into the data directory and
//! restarts the unit `<name>-database`, which reads it on start. It runs on
//! the database's host, or reaches it over the host channel.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::postgres::tls::server_certificate;
use super::super::supabase::owner_vault;
use super::super::DatabaseProvider;

/// `stado database tls reissue NAME`.
#[derive(Debug, clap::Args)]
pub(crate) struct ReissueArgs {
    /// Declared fleet postgres database whose server certificate is issued again.
    pub(in crate::cli::database) name: String,
    /// Emit machine-readable output.
    #[arg(long)]
    pub(in crate::cli::database) json: bool,
}

pub(in crate::cli::database) async fn reissue(args: ReissueArgs) -> Result<(), CmdError> {
    let ReissueArgs {
        name,
        json: json_output,
    } = args;
    let here = crate::cli::release_catalog::this_host().await?;
    let owner = owner_vault::locate().await?;
    let (item, fields) = super::super::dump::item_fields(&name, &owner).await?;
    let provider = super::super::dump::migrate::provider(&item, &fields)?;
    if provider != Some(DatabaseProvider::Fleet) {
        let recorded = match provider {
            Some(provider) => provider.to_string(),
            None => "unset".to_string(),
        };
        return Err(CmdError::usage(format!(
            "{name} is not a fleet database ({item}.provider is {recorded}); only a server the fleet runs has a \
             certificate Stado issued"
        )));
    }
    let host = fields["host"]
        .as_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!("{item} names no host, so no server holds its certificate"))
                .stating(FailureCode::Config)
        })?;
    if host != here {
        let line = format!("database tls reissue {name} --json");
        let arguments: Vec<&str> = line.split_whitespace().collect();
        let output = crate::cli::host::remote_stado_output(host, &arguments)
            .await
            .map_err(|error| {
                let mut wrapped = CmdError::click(format!(
                    "the certificate of {name} was not issued again on {host}: {error}"
                ));
                wrapped.failure = error.failure;
                wrapped
            })?;
        let report = super::last_json(&output).ok_or_else(|| {
            CmdError::click(format!(
                "{name}: stado database tls reissue on {host} printed no report: {}",
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
            CmdError::click(format!("{item} names no address, so no certificate can name it"))
                .stating(FailureCode::Config)
        })?;
    let directory = crate::config_file::home()
        .map_err(|cause| {
            CmdError::click(format!(
                "a fleet database lives under the home, and there is none: {cause}"
            ))
            .stating(FailureCode::Config)
        })?
        .join(".stado")
        .join("databases")
        .join(&name);
    let data = directory.join("data");
    for (what, path) in [("authority", directory.join("ca.crt")), ("authority key", directory.join("ca.key"))] {
        if !crate::primitives::presence::present(&path)? {
            return Err(CmdError::click(format!(
                "{} holds no {what} for {name}, so a new certificate could not be signed by the one consumers \
                 verify against",
                path.display()
            ))
            .stating(FailureCode::Config));
        }
    }
    let policy = crate::config::database_tls::PostgresTls::read().map_err(CmdError::declaration)?;
    server_certificate(&directory, &data, address, &policy)?;
    let unit = format!("{name}-database");
    crate::cli::service::lifecycle::release::unit::restart_quietly(&unit, Some(&here))
        .await
        .map_err(|error| {
            error.within(format!(
                "the new certificate of {name} is written into {}, but {unit} still serves the old one",
                data.display()
            ))
        })?;
    let report = json!({
        "reissued": name,
        "unit": unit,
        "host": here,
        "address": address,
        "certificate": data.join("server.crt").display().to_string(),
        "authority": "unchanged: consumers keep the item's ca_certificate",
    });
    answer(&report, json_output)
}

fn answer(report: &Value, json_output: bool) -> Result<(), CmdError> {
    if json_output {
        println!("{}", serde_json::to_string(report)?);
        return Ok(());
    }
    crate::cli::print_answer(report, false)
}
