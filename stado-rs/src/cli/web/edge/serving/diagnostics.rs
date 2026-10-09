//! What the edge reports: whether its ports answer from the public internet,
//! and how what it terminates compares with what the declarations ask for.

use serde_json::{json, Value};

use super::super::{declared, HOST_UNIT};
use super::{deliver, stado_routes, CmdError};
use crate::config::WebApiEdge;
use crate::deploy::{production_runner, service_serving};

/// Whether the edge answers a TCP connection on one port, from here.
///
/// From this machine deliberately: the operator's laptop is on the same public
/// internet a visitor is, so an answer here is the fact that matters. A
/// loopback check on the edge itself would pass with the security group shut.
async fn answers(address: &str, port: u16) -> (bool, String) {
    let endpoint = format!("{address}:{port}");
    let connected = crate::wait::until(
        crate::wait::Kind::Network,
        "a TCP connection to the edge from this machine",
        endpoint.as_str(),
        tokio::net::TcpStream::connect(&endpoint),
    )
    .await;
    match connected {
        Ok(_) => (true, String::new()),
        Err(error) => (false, error.to_string()),
    }
}

/// Asked only when a public probe got no answer: whether the edge host's Stado
/// process, running its edge role, holds 80 and 443 itself. A proxy holding
/// both while the internet gets no answer is dropped between the two (the
/// edge network's port forwarding or firewall); a proxy holding neither, or a
/// host whose edge role is not running, is the edge's own failure. Without
/// this the status could not tell those apart.
async fn on_edge(edge: &WebApiEdge) -> Value {
    let (target, unit) = match super::delivery::proxy(edge).await {
        Ok(found) => found,
        Err(error) => return json!({ "read": false, "detail": error.to_string() }),
    };
    let read = service_serving::read_serving(
        &target,
        unit.unit_id(),
        &unit.path,
        &[80, 443],
        &production_runner(),
    )
    .await;
    match read {
        Ok(report) => {
            let verdicts = service_serving::port_verdicts(&report);
            let ports: Vec<Value> = verdicts
                .iter()
                .map(|port| {
                    json!({ "port": port.port, "verdict": port.verdict, "holders": port.holder_cell() })
                })
                .collect();
            json!({
                "read": true,
                "serving": service_serving::verdict(&report, &verdicts),
                "ports": ports,
            })
        }
        Err(error) => json!({ "read": false, "detail": error.message }),
    }
}

pub(in crate::cli::web::edge) async fn status(json_output: bool) -> Result<(), CmdError> {
    let edge = declared()?;
    let (http, http_detail) = answers(edge.address(), 80).await;
    let (https, https_detail) = answers(edge.address(), 443).await;
    let routes = stado_routes().await?;
    let terminating: Vec<&String> = routes.iter().map(|(hostname, _)| hostname).collect();
    let local = if http && https {
        Value::Null
    } else {
        on_edge(edge).await
    };
    let report = json!({
        "target": edge.target(),
        "address": edge.address(),
        "contact": edge.contact(),
        "unit": HOST_UNIT,
        "http": { "port":
            80, "answers": http, "detail": http_detail },
        "https": { "port":
            443, "answers": https, "detail": https_detail },
        "on_edge": local,
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
        if local["read"] == true {
            println!(
                "on {} itself: {HOST_UNIT} {} — {}",
                edge.target(),
                local["serving"].as_str().unwrap_or("unknown"),
                local["ports"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|port| format!(
                        "{} {} (holders: {})",
                        port["port"],
                        port["verdict"].as_str().unwrap_or("unknown"),
                        port["holders"].as_str().unwrap_or("-")
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if local["serving"] == service_serving::SERVING_YES {
                println!(
                    "the proxy holds its ports on {} while the internet gets no answer: the edge \
                     network drops the connection before it (port forwarding or firewall for {})",
                    edge.target(),
                    edge.address()
                );
            }
        } else if local["read"] == false {
            println!(
                "on {} itself: could not be read: {}",
                edge.target(),
                local["detail"].as_str().unwrap_or("no detail")
            );
        }
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
