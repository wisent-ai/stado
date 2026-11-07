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
//!
//! A placement made while the host's first SSH destination was its Stado
//! network name (`<host>.stado.internal`) recorded that name as its address
//! and in its URLs, where no resolver answers it: `psql`, `pg_restore` and
//! every consumer's driver fail with `could not translate host name`. Such an
//! item is re-addressed here to the host's address now
//! ([`crate::cli::directory::routable_address`]): its `address`,
//! `pooler_url` and `session_url` name it, the certificate is issued for it,
//! and the unit is installed again when the host no longer manages it (its
//! data and item kept). A `rest_url` or `auth_url` at the old name is named
//! in the answer with the command that places it again, since its unit's own
//! configuration names the server too.
//!
//! What fails, and what is said: an item that is not a fleet database
//! (usage); one naming no host, address or port (config); an address on
//! Stado's network on a host that declares no other destination (config,
//! naming `stado registry host path set`); a missing authority (config); a
//! data directory with no readable `PG_VERSION` (config); the item's write,
//! the unit's installation or its restart, each with what already changed.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::super::supabase::owner_vault;
use super::super::DatabaseProvider;
use super::postgres::tls::server_certificate;

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
            CmdError::click(format!(
                "{item} names no host, so no server holds its certificate"
            ))
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
    let document = owner.document(&item).await?.ok_or_else(|| {
        CmdError::click(format!("{item} is not in {}'s owner vault", owner.name()))
            .stating(FailureCode::NotFound)
    })?;
    let (_, readdressed) = reachable(&name, &item, document, &owner).await?;
    if !readdressed.is_null() {
        // The answer a plain reissue gives, so a reader of it reads this one
        // too, with what the re-address changed beside it.
        let report = json!({
            "reissued": name,
            "unit": readdressed["unit"],
            "host": here,
            "address": readdressed["to"],
            "certificate": readdressed["certificate"],
            "authority": "unchanged: consumers keep the item's ca_certificate",
            "readdressed": readdressed,
        });
        return answer(&report, json_output);
    }
    let address = text(&fields, "address", &item)?;
    let directory = authority(&name)?;
    let data = directory.join("data");
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

/// `document`, the item `item` of fleet database `name`, as a consumer can
/// reach it, and what was changed to make it so (`Null` when nothing was):
/// an address on Stado's network becomes the host's address now, written
/// into `address`, `pooler_url` and `session_url` and into a new server
/// certificate, and a unit the host no longer manages is installed again on
/// the data directory that stands. Runs on the database's host: another host
/// is refused naming where to run it, since the data directory is there.
pub(in crate::cli::database) async fn reachable(
    name: &str,
    item: &str,
    mut document: Value,
    owner: &owner_vault::Owner,
) -> Result<(Value, Value), CmdError> {
    let fields = &document["fields"];
    let host = text(fields, "host", item)?.to_string();
    let address = text(fields, "address", item)?.to_string();
    // A recorded port wider than TCP's names nothing this host can serve.
    let port = match fields["port"].as_u64().map(u16::try_from) {
        Some(Ok(port)) => port,
        Some(Err(_)) | None => {
            return Err(
                CmdError::click(format!("{item} names no TCP port")).stating(FailureCode::Config)
            )
        }
    };
    let target = crate::cli::canonical_host(&host).await?;
    let unit = format!("{name}-database");
    let managed = crate::deploy::service::declared_services(&target)
        .iter()
        .any(|service| service.matches(&unit));
    let networked = crate::network::network_host(&address).is_some();
    if managed && !networked {
        return Ok((document, Value::Null));
    }
    let here = crate::cli::release_catalog::this_host().await?;
    if host != here {
        return Err(CmdError::click(format!(
            "{item} names {address}:{port} on {host}, which {}; run this on {host}, where its data directory is",
            if networked { "no resolver answers" } else { "manages no unit for it" }
        ))
        .stating(FailureCode::Config));
    }
    let readdressed = if networked {
        crate::cli::directory::routable_address(&target).ok_or_else(|| {
            CmdError::click(format!(
                "{item} names {address}, which only Stado's node carries, and {host} declares no other SSH \
                 destination a resolver answers: add one with `stado registry host path set {host} <path> --ssh <user@address>`"
            ))
            .stating(FailureCode::Config)
        })?
    } else {
        address.clone()
    };
    let directory = authority(name)?;
    let data = directory.join("data");
    let policy = crate::config::database_tls::PostgresTls::read().map_err(CmdError::declaration)?;
    server_certificate(&directory, &data, &readdressed, &policy)?;
    // The REST API is placed again first: it mints the secret the sign-in
    // server's configuration takes when it is written again.
    let recorded = |key: &str| {
        fields[key]
            .as_u64()
            .map_or_else(|| format!("<{key}>"), |value| value.to_string())
    };
    let stale: Vec<Value> = [
        (
            "rest_url",
            format!(
                "stado database rest retire {name}, then stado database rest place {name} --port {} \
                 --key-valid-days {}",
                recorded("rest_port"),
                recorded("keys_valid_days")
            ),
        ),
        (
            "auth_url",
            format!(
                "stado database auth place {name} --port {} --reconfigure --ready-within <seconds>",
                recorded("auth_port")
            ),
        ),
    ]
    .into_iter()
    .filter(|(field, _)| {
        networked && fields[*field].as_str().is_some_and(|url| url.contains(&address))
    })
    .map(|(field, place_again)| {
        json!({
            "field": field,
            "names": address,
            "place_again": place_again,
        })
    })
    .collect();
    if networked {
        let written = document["fields"].as_object_mut().ok_or_else(|| {
            CmdError::click(format!("{item} holds no fields object")).stating(FailureCode::Config)
        })?;
        written.insert("address".to_string(), json!(readdressed));
        for field in ["pooler_url", "session_url"] {
            let Some(url) = written
                .get(field)
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            written.insert(
                field.to_string(),
                json!(rehosted(item, field, &url, &readdressed)?),
            );
        }
        owner
            .store(item, "bundle", &document["fields"], &document["context"])
            .await
            .map_err(|error| {
                error.within(format!("the certificate of {name} names {readdressed}, but {item} still names {address}"))
            })?;
    }
    let unit_status = if managed {
        crate::cli::service::lifecycle::release::unit::restart_quietly(&unit, Some(&here))
            .await
            .map_err(|error| {
                error.within(format!(
                    "{item} names {readdressed}, but {unit} still serves the old certificate"
                ))
            })?;
        "restarted"
    } else {
        let major = std::fs::read_to_string(data.join("PG_VERSION")).map_err(|error| {
            CmdError::click(format!(
                "{} holds no readable PG_VERSION: {error}",
                data.display()
            ))
            .stating(FailureCode::Config)
        })?;
        let postgres = crate::cli::database::dump::tools::program("postgres", Some(major.trim()))?;
        super::postgres::serve(&unit, &here, &postgres, &data, port)
            .await
            .map_err(|error| {
                error.within(format!(
                    "{item} names {readdressed}, but the unit {unit} was not installed again"
                ))
            })?;
        "installed again"
    };
    let report = json!({
        "item": item,
        "from": address,
        "to": readdressed,
        "port": port,
        "unit": unit,
        "unit_status": unit_status,
        "certificate": data.join("server.crt").display().to_string(),
        "stale": stale,
    });
    Ok((document, report))
}

/// `url` (the item's `field`) naming `address` as its host.
fn rehosted(item: &str, field: &str, url: &str, address: &str) -> Result<String, CmdError> {
    let refused = |what: String| {
        CmdError::click(format!("{item}#{field} {what}")).stating(FailureCode::Config)
    };
    let mut parsed =
        url::Url::parse(url).map_err(|error| refused(format!("is not a URL: {error}")))?;
    let host = if address.contains(':') {
        format!("[{address}]")
    } else {
        address.to_string()
    };
    parsed
        .set_host(Some(&host))
        .map_err(|error| refused(format!("cannot name {address}: {error}")))?;
    Ok(parsed.to_string())
}

/// The directory of fleet database `name` on this host, refused when it holds
/// no authority to sign a certificate consumers verify against.
fn authority(name: &str) -> Result<std::path::PathBuf, CmdError> {
    let directory = crate::config_file::home()
        .map_err(|cause| {
            CmdError::click(format!(
                "a fleet database lives under the home, and there is none: {cause}"
            ))
            .stating(FailureCode::Config)
        })?
        .join(".stado")
        .join("databases")
        .join(name);
    for (what, path) in [
        ("authority", directory.join("ca.crt")),
        ("authority key", directory.join("ca.key")),
    ] {
        if !crate::primitives::presence::present(&path)? {
            return Err(CmdError::click(format!(
                "{} holds no {what} for {name}, so a new certificate could not be signed by the one consumers \
                 verify against",
                path.display()
            ))
            .stating(FailureCode::Config));
        }
    }
    Ok(directory)
}

fn text<'a>(fields: &'a Value, key: &str, item: &str) -> Result<&'a str, CmdError> {
    fields[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!("{item} names no {key}")).stating(FailureCode::Config)
        })
}

fn answer(report: &Value, json_output: bool) -> Result<(), CmdError> {
    if json_output {
        println!("{}", serde_json::to_string(report)?);
        return Ok(());
    }
    crate::cli::print_answer(report, false)
}
