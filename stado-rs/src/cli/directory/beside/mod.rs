//! A service this host runs for its own callers beside the placement the
//! fleet directory names elsewhere.
//!
//! The directory places each service on one active host, and every other host
//! reaches it through its own resolver adapter. When the host it is placed on
//! cannot be reached, or the resolver cannot start, every caller on this host
//! loses the service, even when this host runs an instance of it itself: a
//! laptop whose Brama route went through the mac mini lost its push review
//! while the mini was out of reach. The fleet registry cannot record the fix
//! from such a host, since the registry's authority is the host out of reach.
//!
//! So a host states, in a file of its own (`~/.stado/beside.json`), which
//! services it runs beside their placement and at which loopback address:
//! `stado service directory beside <service> --url <url>` declares one,
//! `--withdraw` takes it back, and `stado service directory beside` lists
//! them. `stado service directory connect` on this host then answers that
//! address, saying `placement: beside` and naming this host instead of the
//! active one; nothing falls back silently. The statement is this host's
//! alone: another host's view (`--target`) and the registry are unchanged.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cli::CmdError;

/// One service this host runs beside its placement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Local {
    pub(crate) url: String,
    pub(crate) declared_at: String,
}

fn path() -> PathBuf {
    crate::config_file::expand_tilde("~/.stado/beside.json")
}

/// Every service this host declares beside its placement. No file is none;
/// a file that cannot be read or parsed is refused naming it.
pub(crate) fn read() -> Result<BTreeMap<String, Local>, CmdError> {
    let path = path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(CmdError::click(format!(
                "this host's beside placements in {} cannot be read: {error}",
                path.display()
            )))
        }
    };
    serde_json::from_str(&text).map_err(|error| {
        CmdError::declaration(format!(
            "this host's beside placements in {} are not a placement document: {error}",
            path.display()
        ))
    })
}

/// The address this host runs `service` at beside its placement, if it does.
pub(crate) fn local_url(service: &str) -> Result<Option<String>, CmdError> {
    Ok(read()?.remove(service).map(|local| local.url))
}

fn write(placements: &BTreeMap<String, Local>) -> Result<(), CmdError> {
    let path = path();
    let body = serde_json::to_vec_pretty(placements).map_err(|error| {
        CmdError::click(format!("beside placements cannot be serialized: {error}"))
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CmdError::click(format!(
                "the directory of {} cannot be made: {error}",
                path.display()
            ))
        })?;
    }
    let staged = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&staged, body)
        .and_then(|()| std::fs::rename(&staged, &path))
        .map_err(|error| {
            CmdError::click(crate::primitives::staging::failed_with(
                format!(
                    "this host's beside placements could not be written to {}: {error}",
                    path.display()
                ),
                &staged,
            ))
        })
}

/// A loopback `http(s)://host:port` URL, or the refusal naming what it is not.
fn loopback(url: &str) -> Result<String, CmdError> {
    let parsed = url::Url::parse(url.trim())
        .map_err(|error| CmdError::usage(format!("--url {url:?} is not a URL: {error}")))?;
    let local = match parsed.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    };
    if !local || parsed.port().is_none() {
        return Err(CmdError::usage(format!(
            "--url {url:?} must name a loopback address with its port: a service run beside its \
             placement serves this host's own callers"
        )));
    }
    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

fn print(placements: &BTreeMap<String, Local>, as_json: bool) -> Result<(), CmdError> {
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "beside": placements }))?
        );
    } else if placements.is_empty() {
        println!("this host runs no service beside its placement");
    } else {
        for (service, local) in placements {
            println!(
                "{service}  {}  (beside its placement, declared {})",
                local.url, local.declared_at
            );
        }
    }
    Ok(())
}

/// `stado service directory beside [<service> (--url <url> | --withdraw)]`.
pub(in crate::cli::directory) fn run(
    service: Option<String>,
    url: Option<String>,
    withdraw: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    let mut placements = read()?;
    let Some(service) = service else {
        if url.is_some() || withdraw {
            return Err(CmdError::usage(
                "--url and --withdraw name a service: stado service directory beside <service> ...",
            ));
        }
        return print(&placements, as_json);
    };
    match (url, withdraw) {
        (Some(_), true) => Err(CmdError::usage("--url declares a placement and --withdraw takes one back; give one")),
        (None, false) => Err(CmdError::usage(format!(
            "stado service directory beside {service} needs --url <loopback url> to declare it or --withdraw to take it back"
        ))),
        (Some(url), false) => {
            let url = loopback(&url)?;
            placements
                .insert(service, Local { url, declared_at: chrono::Utc::now().to_rfc3339() });
            write(&placements)?;
            print(&placements, as_json)
        }
        (None, true) => {
            if placements.remove(&service).is_none() {
                return Err(CmdError::missing(format!(
                    "this host declares no {service} beside its placement; `stado service directory beside` lists what it declares"
                )));
            }
            write(&placements)?;
            print(&placements, as_json)
        }
    }
}
