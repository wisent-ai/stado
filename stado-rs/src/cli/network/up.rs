//! `stado network up`: join this host to Stado's network and make it the
//! host's preferred path. Its promise is kept in this order, and each step
//! is observed before the next: the key and the node's port exist; the
//! registry declares the node; the node's unit runs and publishes that it
//! listens; this host's own SSH server answers through the node at a
//! declared endpoint, under the registry key; only then does the registry
//! make the network the primary path, so a node that never answered is
//! never anybody's preferred path.
//!
//! The key and the port come first because neither needs the registry
//! authority. The key is kept whatever the read answers, so a host that
//! cannot read the authority yet declares the same identity on its next
//! `up` instead of minting another.

use clap::Args;
use serde_json::json;
use tokio::io::AsyncReadExt;

use crate::cli::registry::commit_document;
use crate::cli::CmdError;
use crate::network::node::{declaration, serving, State};
use crate::network::{key, NetworkNode, Relay};

#[derive(Debug, Args)]
pub struct UpArgs {
    /// The registry host to join; this machine's own registry entry when
    /// omitted. Another machine is joined by its own Stado, over its host
    /// channel.
    #[arg(long)]
    host: Option<String>,
    /// A reachable `host:port` peers on other networks dial (a port this
    /// host's router forwards to the node, or a public address). Repeatable.
    #[arg(long = "endpoint", value_name = "HOST:PORT")]
    endpoints: Vec<String>,
    /// Declare none of the endpoints measured on this host's interfaces: a
    /// host behind NAT that peers reach through a relay only.
    #[arg(long)]
    no_direct: bool,
    /// Relay for hosts peers cannot dial. A relay needs an endpoint every
    /// host can reach.
    #[arg(long, requires = "attach_retry_seconds")]
    relay: bool,
    /// With --relay: how long a node whose attachment to this relay ended
    /// waits before attaching again.
    #[arg(long, value_name = "SECONDS", requires = "relay")]
    attach_retry_seconds: Option<u64>,
    /// Emit machine-readable output.
    #[arg(long)]
    json: bool,
}

fn endpoint_checked(endpoint: &str) -> Result<String, CmdError> {
    match endpoint.rsplit_once(':') {
        Some((host, port))
            if !host.is_empty() && port.parse::<u16>().is_ok_and(|port| port != 0) =>
        {
            Ok(endpoint.to_string())
        }
        _ => Err(CmdError::usage(format!(
            "--endpoint {endpoint:?} is not HOST:PORT with a nonzero port"
        ))),
    }
}

/// The port this account's node listens on: the one already recorded, else
/// one the system hands out now. One account runs one node, so the port is
/// the account's whichever registry host it is declared as.
fn node_port() -> Result<u16, CmdError> {
    if let Some(state) = State::load().map_err(CmdError::declaration)? {
        return Ok(state.port);
    }
    let listener =
        std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).map_err(|error| {
            CmdError::unreachable(format!(
                "this host hands out no port for the network node: {error}"
            ))
        })?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| {
            CmdError::unreachable(format!("the handed-out port cannot be read back: {error}"))
        })
}

pub(super) async fn up(args: UpArgs) -> Result<(), CmdError> {
    if args.attach_retry_seconds == Some(0) {
        return Err(CmdError::usage("--attach-retry-seconds must be at least 1"));
    }
    let declared_endpoints = args
        .endpoints
        .iter()
        .map(|endpoint| endpoint_checked(endpoint))
        .collect::<Result<Vec<_>, _>>()?;
    // `--host` is named against what this machine holds of the registry,
    // before anything is minted: a host it does not declare is refused with
    // nothing left behind, and another machine is joined by its own Stado.
    if let Some(host) = args.host.as_deref() {
        if let Ok((copy, notice)) =
            crate::targets::registry_without_authority("naming the host `network up` joins").await
        {
            if copy.lookup(host).is_none() {
                return Err(CmdError::missing(match notice {
                    None => format!("the registry declares no host {host}"),
                    Some(notice) => format!("the registry declares no host {host} ({notice})"),
                }));
            }
            if let Some(other) = super::another_machine(&copy, host) {
                return forwarded(&other, &args).await;
            }
        }
    }
    let (keypair, created) = key::load_or_create().map_err(CmdError::declaration)?;
    let port = node_port()?;
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| {
            CmdError::unreachable(format!(
                "`network up` declares the node in the registry, whose authority did not answer: \
                 {error}; this host's key {} is kept, and the next `network up` declares it",
                keypair.public_base64()
            ))
        })?;
    let host = super::this_host(&registry, args.host.as_deref())?;
    let measured = declaration::measure(port)
        .await
        .map_err(CmdError::unreachable)?;
    let mut endpoints = if args.no_direct {
        Vec::new()
    } else {
        measured.endpoints.clone()
    };
    endpoints.extend(declared_endpoints);
    if endpoints.is_empty()
        && (args.relay
            || crate::network::peers::relays(&registry, &[host.as_str()])
                .next()
                .is_none())
    {
        return Err(CmdError::refused(format!(
            "{host} would declare no endpoint and no relay could carry it: this host measured none on its interfaces{}; `--endpoint HOST:PORT` declares a reachable one",
            if args.no_direct { " that --no-direct keeps" } else { "" }
        )));
    }
    let node = NetworkNode {
        public_key: keypair.public_base64(),
        endpoints,
        relay: args.attach_retry_seconds.map(|attach_retry_seconds| Relay {
            attach_retry_seconds,
        }),
    };

    // 1. The node, declared without moving any path.
    let declared_node = node.clone();
    let declared_host = host.clone();
    commit_document(move |document| {
        let mut next = document.clone();
        let entry = next
            .get_mut("targets")
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|targets| {
                targets.iter_mut().find(|target| {
                    target.get("name").and_then(serde_json::Value::as_str)
                        == Some(declared_host.as_str())
                })
            })
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::missing(format!(
                    "the registry no longer declares host {declared_host}"
                ))
            })?;
        entry.insert("network".to_string(), serde_json::to_value(&declared_node)?);
        Ok(next)
    })
    .await?;
    // The node and every reader here read this host's copy, so it is
    // refreshed from the authority that just took the declaration.
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| {
            CmdError::unreachable(format!(
                "the node is declared but the registry copy could not be refreshed: {error}"
            ))
        })?;
    let target = registry
        .lookup(&host)
        .cloned()
        .ok_or_else(|| CmdError::missing(format!("the registry no longer declares host {host}")))?;

    // 2. The node runs and listens.
    State {
        host: host.clone(),
        port,
    }
    .store()
    .map_err(CmdError::declaration)?;
    let before = serving::read().map_err(CmdError::declaration)?;
    let unit = super::unit::install(&target).await?;
    let published = serving::await_serving(port, before.as_ref()).map_err(CmdError::unreachable)?;

    // 3. This host's own SSH server answers through it.
    let ssh_port = match crate::network::peers::ssh_ports(&target).first() {
        Some(port) => *port,
        None => {
            return Err(CmdError::declaration(format!(
                "registry host {host} declares no SSH path whose port the node would open"
            )))
        }
    };
    let banner = ssh_banner(&host, ssh_port).await?;

    // 4. Only now the network becomes the preferred path.
    let (node_for_paths, lan) = (node.clone(), measured.lan.clone());
    let path_host = host.clone();
    let outcome = std::sync::Arc::new(std::sync::Mutex::new(None));
    let recorded = std::sync::Arc::clone(&outcome);
    let generation = commit_document(move |document| {
        let mut next = document.clone();
        let declared = declaration::declare(&mut next, &path_host, &node_for_paths, lan.as_deref())
            .map_err(CmdError::declaration)?;
        if let Ok(mut slot) = recorded.lock() {
            *slot = Some(declared);
        }
        Ok(next)
    })
    .await?;
    // A refresh failing here leaves the copy one write behind, which the
    // next registry read of any command on this host repairs; it is said.
    if let Err(error) = crate::targets::fetch_registry_remote().await {
        eprintln!(
            "stado network: the registry copy was not refreshed after the path change: {error}"
        );
    }
    let declared = outcome.lock().ok().and_then(|mut slot| slot.take());
    let receipt = json!({
        "host": host,
        "public_key": node.public_key,
        "key_created": created,
        "key_file": key::path().map_err(CmdError::declaration)?.display().to_string(),
        "port": published.port,
        "pid": published.pid,
        "endpoints": node.endpoints,
        "relay": node.relay,
        "unit": unit,
        "ssh_answered": banner,
        "paths": declared,
        "generation": generation,
    });
    print_receipt(&receipt, args.json)
}

/// `network up` run on `host` by that host's own Stado over its host
/// channel: the node, its key and its unit are that host's. Its receipt is
/// said here as it would be there.
async fn forwarded(host: &str, args: &UpArgs) -> Result<(), CmdError> {
    let attach_retry = args.attach_retry_seconds.map(|seconds| seconds.to_string());
    let mut arguments = vec!["network", "up", "--host", host, "--json"];
    for endpoint in &args.endpoints {
        arguments.extend(["--endpoint", endpoint.as_str()]);
    }
    if args.no_direct {
        arguments.push("--no-direct");
    }
    if args.relay {
        arguments.push("--relay");
    }
    if let Some(seconds) = attach_retry.as_deref() {
        arguments.extend(["--attach-retry-seconds", seconds]);
    }
    let stdout = crate::cli::host::remote_stado_output(host, &arguments).await?;
    let receipt: serde_json::Value = serde_json::from_str(&stdout).map_err(|error| {
        CmdError::unreachable(format!(
            "`stado network up` on {host} answered with something that is not its receipt \
             ({error}): {}",
            stdout.trim()
        ))
    })?;
    print_receipt(&receipt, args.json)
}

/// Say a receipt: the document itself with `--json`, else its lines.
fn print_receipt(receipt: &serde_json::Value, json: bool) -> Result<(), CmdError> {
    if json {
        println!("{}", serde_json::to_string_pretty(receipt)?);
        return Ok(());
    }
    let field = |key: &str| match receipt.get(key) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    let endpoints: Vec<&str> = receipt
        .get("endpoints")
        .and_then(serde_json::Value::as_array)
        .map(|endpoints| {
            endpoints
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect()
        })
        .unwrap_or_default();
    println!(
        "{} is on the Stado network: node pid {} on port {}, key {}",
        field("host"),
        field("pid"),
        field("port"),
        field("public_key")
    );
    println!(
        "  endpoints: {}",
        if endpoints.is_empty() {
            "none (reached through a relay)".to_string()
        } else {
            endpoints.join(", ")
        }
    );
    println!(
        "  its own SSH server answered through the node: {}",
        field("ssh_answered")
    );
    if let Some(paths) = receipt.get("paths").filter(|paths| !paths.is_null()) {
        println!(
            "  registry paths: {paths}; generation={}",
            field("generation")
        );
    }
    Ok(())
}

/// The SSH identification line this host's server sends through its own
/// node, reached at a declared endpoint under the registry key.
async fn ssh_banner(host: &str, port: u16) -> Result<String, CmdError> {
    let mut stream = crate::network::dial(host, port).await.map_err(|cause| {
        CmdError::unreachable(format!(
            "the node runs but does not carry this host's own SSH server: {cause}"
        ))
    })?;
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    // RFC 4253 4.2: the identification string ends in CR LF and is at most
    // 255 characters including them.
    while line.len() < 255 {
        match stream.read(&mut byte).await {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => line.push(byte[0]),
            Err(error) => {
                return Err(CmdError::unreachable(format!(
                    "the SSH server's greeting through the node broke: {error}"
                )))
            }
        }
    }
    let banner = String::from_utf8_lossy(&line).trim_end().to_string();
    if banner.starts_with("SSH-") {
        Ok(banner)
    } else {
        Err(CmdError::unreachable(format!(
            "port {port} answered through the node, but not as an SSH server: {banner:?}"
        )))
    }
}
