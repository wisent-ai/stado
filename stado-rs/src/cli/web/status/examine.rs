//! The precedence between the four facts: which one being wrong names the
//! thing to repair. The readers gather, this decides, and the order the
//! branches are written in is the order a hosted product stops working in.

use serde_json::{json, Value};

use super::readers::{expected_address, resolve_hostname, upstream_service_state};
use super::{
    Verdict, DNS_TIMEOUT, DNS_UNREADABLE, DNS_UNRESOLVED, VERDICT_DNS_ELSEWHERE,
    VERDICT_EDGE_UNCONFIGURED, VERDICT_NOT_DEPLOYED, VERDICT_PORT_UNHELD, VERDICT_SERVING,
    VERDICT_UNIT_DOWN,
};
use crate::config::WebApiProduct;
use crate::deploy::service::{self, ServiceStatus};
use crate::deploy::{host_channel, service_serving, Runner};

/// Everything one product's report needs, gathered from the four readers.
pub(super) async fn examine(
    name: &str,
    declared: &WebApiProduct,
    managed: Option<&ServiceStatus>,
    managed_all: &[ServiceStatus],
    runner: &Runner,
) -> Verdict {
    let (expected, edge_error) = match expected_address(declared) {
        Ok(expected) => (expected, None),
        Err(problems) => (None, Some(problems.join("; "))),
    };
    // A redirect has no unit, no port and no host to ask. Its whole state is
    // whether the hostname resolves to the edge that answers it, and running
    // the unit questions against it would report `not-deployed` for something
    // that was never deployable.
    if declared.is_redirect() {
        let (dns_state, addresses) = resolve_hostname(declared.hostname()).await;
        let published =
            expected.is_some_and(|address| addresses.iter().any(|found| found == address));
        let word = if edge_error.is_some() {
            VERDICT_EDGE_UNCONFIGURED
        } else if published {
            VERDICT_SERVING
        } else {
            VERDICT_NOT_DEPLOYED
        };
        return Verdict {
            row: json!({
                "product": name,
                "verdict": word,
                "kind": "redirect",
                "redirect_to": declared.redirect_to(),
                "hostname": declared.hostname(),
                "edge": declared.edge(),
                "edge_error": edge_error,
                "dns": dns_state,
                "addresses": addresses,
                "expected_address": expected,
            }),
            word,
        };
    }
    // A hostname in front of an existing service has no unit of its own
    // either. Its two questions are whether the hostname resolves to the edge
    // and whether the service behind it is up — and the second is answered by
    // the service plane, which is the only thing that knows, rather than by
    // asking a host about `com.wisent.web.<product>`, a unit that does not
    // exist and never will.
    if let Some(service) = declared.upstream_service() {
        let (dns_state, addresses) = resolve_hostname(declared.hostname()).await;
        let published =
            expected.is_some_and(|address| addresses.iter().any(|found| found == address));
        let upstream = upstream_service_state(service, managed_all);
        let up = upstream
            .get("state")
            .and_then(Value::as_str)
            .is_some_and(|state| state == service::STATE_ACTIVE);
        let word = if !up {
            VERDICT_UNIT_DOWN
        } else if edge_error.is_some() {
            VERDICT_EDGE_UNCONFIGURED
        } else if published {
            VERDICT_SERVING
        } else {
            VERDICT_NOT_DEPLOYED
        };
        return Verdict {
            row: json!({
                "product": name,
                "verdict": word,
                "kind": "upstream-service",
                "upstream_service": service,
                "upstream": upstream,
                "hostname": declared.hostname(),
                "edge": declared.edge(),
                "edge_error": edge_error,
                "dns": dns_state,
                "addresses": addresses,
                "expected_address": expected,
            }),
            word,
        };
    }
    let unit = super::unit_label(name);
    let port = declared.port();
    let (dns_state, addresses) = resolve_hostname(declared.hostname()).await;

    // The port question is asked on the host, and only when there is a unit
    // to ask about. A host read for a product the registry does not manage
    // would spend an ssh connection to learn what the document already said,
    // and a host read for a unit the beacon reports down would report its port
    // dead as if that were news.
    let mut port_state = "unasked".to_string();
    let mut port_detail = String::new();
    let mut holders: Vec<String> = Vec::new();

    let unit_state = managed
        .map(|row| row.state.clone())
        .unwrap_or_else(|| "undeclared".to_string());
    let reported_at = managed
        .map(|row| row.reported_at.clone())
        .unwrap_or_default();

    let mut word = if managed.is_none() {
        VERDICT_NOT_DEPLOYED
    } else if unit_state != service::STATE_ACTIVE {
        VERDICT_UNIT_DOWN
    } else {
        VERDICT_SERVING
    };

    if word == VERDICT_SERVING {
        let managed = managed.expect("an active row exists to have been judged active");
        match host_channel::canonical_target(&managed.service.host).await {
            Ok(target) => {
                match service_serving::read_serving(
                    &target,
                    managed.service.unit_id(),
                    &managed.service.path,
                    &[port],
                    runner,
                )
                .await
                {
                    Ok(report) => {
                        let verdicts = service_serving::port_verdicts(&report);
                        let serving = service_serving::verdict(&report, &verdicts);
                        holders = verdicts
                            .iter()
                            .flat_map(|verdict| verdict.holders.iter())
                            .map(|holder| {
                                format!(
                                    "{} ({}) owned by {}",
                                    holder.pid,
                                    holder.comm,
                                    if holder.owner.is_empty() {
                                        "an unreadable job"
                                    } else {
                                        holder.owner.as_str()
                                    }
                                )
                            })
                            .collect();
                        port_state = verdicts
                            .first()
                            .map(|verdict| verdict.verdict.to_string())
                            .unwrap_or_else(|| service_serving::PORT_UNKNOWN.to_string());
                        if serving != service_serving::SERVING_YES {
                            word = VERDICT_PORT_UNHELD;
                            port_detail =
                                service_serving::failure(&managed.service.host, &report, &verdicts)
                                    .unwrap_or_else(|| {
                                        format!("port {port} is not held by {unit}")
                                    });
                        }
                    }
                    // The host could not be asked. That is not a passing
                    // check: a control plane that reported "cannot tell" as
                    // healthy is the exact defect `service_serving` was
                    // written after.
                    Err(error) => {
                        word = VERDICT_PORT_UNHELD;
                        port_state = service_serving::PORT_UNKNOWN.to_string();
                        port_detail = format!(
                            "whether {unit} holds port {port} could not be established: {error}"
                        );
                    }
                }
            }
            Err(error) => {
                word = VERDICT_PORT_UNHELD;
                port_state = service_serving::PORT_UNKNOWN.to_string();
                port_detail = format!(
                    "{} could not be resolved as a registry target, so its port was not judged: \
                     {error}",
                    managed.service.host
                );
            }
        }
    }

    // DNS is judged last because it is the only failure that leaves a working
    // unit: a product whose unit is down is reported as that, not as a name
    // pointing at the wrong place.
    let dns_detail = match (expected, dns_state) {
        (_, DNS_UNREADABLE) => format!(
            "{} could not be resolved inside {}s",
            declared.hostname(),
            DNS_TIMEOUT.as_secs()
        ),
        (_, DNS_UNRESOLVED) => format!("{} resolves to no address", declared.hostname()),
        (Some(address), _) if !addresses.iter().any(|found| found == address) => format!(
            "{} points at {} and this product's edge is {address}",
            declared.hostname(),
            addresses.join(", ")
        ),
        (Some(address), _) => format!("{} points at the edge {address}", declared.hostname()),
        (None, _) => match &edge_error {
            Some(problem) => format!(
                "{} resolves to {}, but {problem}",
                declared.hostname(),
                addresses.join(", ")
            ),
            None => format!(
                "{} points at {} through the {} edge, whose addresses are not ours to assert",
                declared.hostname(),
                addresses.join(", "),
                declared.edge()
            ),
        },
    };
    let dns_wrong = match (expected, dns_state) {
        (_, DNS_UNREADABLE | DNS_UNRESOLVED) => true,
        (Some(address), _) => !addresses.iter().any(|found| found == address),
        (None, _) => false,
    };
    if word == VERDICT_SERVING {
        if edge_error.is_some() {
            word = VERDICT_EDGE_UNCONFIGURED;
        } else if dns_wrong {
            word = VERDICT_DNS_ELSEWHERE;
        }
    }

    let row = json!({
        "product": name,
        "verdict": word,
        "host": declared.host(),
        "port": port,
        "hostname": declared.hostname(),
        "unit": unit,
        "unit_domain": super::UNIT_DOMAIN,
        "edge": declared.edge(),
        "edge_error": edge_error,
        "consumer": declared.consumer(),
        "readyz": declared.readyz(),
        "unit_state": unit_state,
        "unit_reported_at": reported_at,
        "unit_detail": managed.map(|row| row.detail.clone()).unwrap_or_default(),
        "managed_unit": managed.map(|row| row.service.unit_id().to_string()),
        "port_state": port_state,
        "port_detail": port_detail,
        "port_holders": holders,
        "dns_state": dns_state,
        "dns_addresses": addresses,
        "dns_expected": expected,
        "dns_detail": dns_detail,
    });
    Verdict { row, word }
}
