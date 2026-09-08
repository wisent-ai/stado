//! What the edge reports: whether its ports answer from the public internet,
//! and how what it terminates compares with what the declarations ask for.

use serde_json::{json, Value};

use super::super::{declared, PROBE_TIMEOUT, PROXY_UNIT};
use super::{deliver, stado_routes, CmdError};

/// Whether the edge answers a TCP connection on one port, from here.
///
/// From this machine deliberately: the operator's laptop is on the same public
/// internet a visitor is, so an answer here is the fact that matters. A
/// loopback check on the edge itself would pass with the security group shut.
async fn answers(address: &str, port: u16) -> (bool, String) {
    let endpoint = format!("{address}:{port}");
    match tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect(&endpoint)).await {
        Ok(Ok(_)) => (true, String::new()),
        Ok(Err(error)) => (false, error.to_string()),
        Err(_) => (
            false,
            format!("no answer within {}s", PROBE_TIMEOUT.as_secs()),
        ),
    }
}

pub(in crate::cli::web::edge) async fn status(json_output: bool) -> Result<(), CmdError> {
    let edge = declared()?;
    let (http, http_detail) = answers(edge.address(), 80).await;
    let (https, https_detail) = answers(edge.address(), 443).await;
    let routes = stado_routes().await?;
    let terminating: Vec<&String> = routes.iter().map(|(hostname, _)| hostname).collect();
    let report = json!({
        "target": edge.target(),
        "address": edge.address(),
        "contact": edge.contact(),
        "unit": super::unit_label(PROXY_UNIT),
        "http": { "port":
            80, "answers": http, "detail": http_detail },
        "https": { "port":
            443, "answers": https, "detail": https_detail },
        "hostnames": terminating,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let word = |answered: bool, detail: &str| {
            if answered {
                "answers".to_string()
            } else {
                format!("no answer ({detail})")
            }
        };
        println!(
            "{} at {}: 80 {}, 443 {}",
            edge.target(),
            edge.address(),
            word(http, &http_detail),
            word(https, &https_detail),
        );
        if terminating.is_empty() {
            println!("declared to terminate: nothing");
        } else {
            println!(
                "declared to terminate: {}",
                terminating
                    .iter()
                    .map(|hostname| hostname.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    Ok(())
}

/// The reconcile, read-only.
///
/// Exits non-zero when the two sets differ, the way `stado dns set --check`
/// does: a reconcile report that returns success while the edge is missing a
/// hostname cannot be used as a gate, and being usable as a gate is the point
/// of reporting both sets rather than just fixing them.
pub(in crate::cli::web::edge) async fn hostnames(json_output: bool) -> Result<(), CmdError> {
    let edge = declared()?;
    let routes = stado_routes().await?;
    let report = deliver(edge, &routes, false).await?;
    let reconciled = report["change"].as_str() == Some("unchanged");
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let names = |key: &str| {
            report[key]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|joined| !joined.is_empty())
                .unwrap_or_else(|| "none".to_string())
        };
        println!("must terminate: {}", names("hostnames"));
        println!("terminates now: {}", names("terminated"));
        println!("missing: {}", names("missing"));
        println!("unexpected: {}", names("unexpected"));
    }
    if reconciled {
        Ok(())
    } else {
        Err(CmdError::silent(1))
    }
}
