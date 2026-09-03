//! `stado web status` — one verdict per declared web product.
//!
//! Four facts, in the order a hosted product stops working in, and each read
//! from the thing that actually knows it:
//!
//! 1. **What the product declares** — host, port, hostname, unit — out of the
//!    configuration plane, so the report says what is supposed to be true
//!    before it says what is.
//! 2. **The unit's live state** — from the health beacons through
//!    [`crate::deploy::service::list_services`], the same join
//!    `stado service list` and `stado service status` answer from. Beacon-only
//!    by construction: the moment you most need to know what is supposed to be
//!    running on a host is the moment the host has stopped answering, so this
//!    half costs no ssh at all.
//! 3. **Whether the declared port is held by that unit** — from the host,
//!    through [`crate::deploy::service_serving`], because that is the one
//!    question a declaration cannot answer about itself. `service show` says
//!    `runs` whenever the unit FILE exists, and a mac mini spent days with a
//!    dead unit reported healthy while a different launchd job held its port.
//! 4. **What the hostname actually resolves to** — because a unit that serves
//!    perfectly behind a record pointing at a retired edge is an outage, and
//!    it is the one failure every other reader here is blind to.
//!
//! The overall word is the first of those that is wrong, so it names the thing
//! to repair rather than the last symptom. A product that is not `serving`
//! makes the command exit non-zero: a status command that reports a broken
//! product and exits zero is a status command nothing can gate on.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use super::CmdError;
use crate::config::WebApiProduct;
use crate::deploy::service::{self, ServiceStatus};
use crate::deploy::{host_channel, production_runner, service_serving, Runner};

/// The unit is loaded, its declared port is held by its own process, and the
/// hostname resolves where this product's edge is.
const VERDICT_SERVING: &str = "serving";
/// Nothing in the registry manages this product's unit yet. `stado web
/// declare` records a product; `stado web deploy` is what makes it run.
const VERDICT_NOT_DEPLOYED: &str = "not-deployed";
/// The unit is managed and the host says it is not running.
const VERDICT_UNIT_DOWN: &str = "unit-down";
/// The unit is loaded and its own declared port is not held by its own
/// process — dead, taken by another job, or unreadable.
const VERDICT_PORT_UNHELD: &str = "port-unheld";
/// The unit serves and the public hostname does not point at this product's
/// edge, so the public name reaches something else or nothing at all.
const VERDICT_DNS_ELSEWHERE: &str = "dns-elsewhere";

/// The hostname resolved to at least one address.
const DNS_RESOLVED: &str = "resolved";
/// The resolver answered and the name has no address.
const DNS_UNRESOLVED: &str = "unresolved";
/// The resolver did not answer inside the window. Deliberately not folded
/// into [`DNS_UNRESOLVED`]: "this name has no address" and "nobody could ask"
/// are opposite findings, and only one of them is the product's fault.
const DNS_UNREADABLE: &str = "unreadable";

/// How long one hostname lookup may take.
///
/// Bounded because a status read over every declared product must not hang on
/// one unreachable resolver, and short because a name that needs longer than
/// this is already the finding. Two seconds is the window `doctor.rs` puts
/// around its own reachability lookups.
const DNS_TIMEOUT: Duration = Duration::from_secs(2);

/// Resolve one public hostname to its addresses.
///
/// This uses `tokio::net::lookup_host` — the host's own stub resolver, through
/// the standard library — rather than a DNS client of this crate's own. There
/// is no resolver machinery here to reuse: `cli/dns.rs` speaks Namecheap's
/// zone API and answers "what does the zone say", which is a different
/// question and would go on answering correctly while a record served from a
/// stale cache pointed somewhere else. `doctor.rs` already reaches for
/// `lookup_host` under a timeout for exactly this reason, and this follows it,
/// so what is reported is what a browser would actually get.
///
/// The port in the query is `443` because `lookup_host` resolves a socket
/// address and needs one; it is discarded, and nothing here connects.
async fn resolve_hostname(hostname: &str) -> (&'static str, Vec<String>) {
    let lookup = tokio::time::timeout(
        DNS_TIMEOUT,
        tokio::net::lookup_host(format!("{hostname}:443")),
    )
    .await;
    match lookup {
        Ok(Ok(addresses)) => {
            let mut found: Vec<String> = addresses
                .map(|address| address.ip().to_string())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            found.dedup();
            if found.is_empty() {
                (DNS_UNRESOLVED, found)
            } else {
                (DNS_RESOLVED, found)
            }
        }
        // A resolver that answers "no such name" and a resolver that cannot be
        // reached both arrive here as an error from the same call, and the
        // stub resolver does not distinguish them for us. Reported as
        // unresolved with the reason in `dns_detail`, never as an address.
        Ok(Err(_)) => (DNS_UNRESOLVED, Vec::new()),
        Err(_) => (DNS_UNREADABLE, Vec::new()),
    }
}

/// Where this product's hostname is supposed to point, when that is knowable.
///
/// For the Stado edge it is the edge host's public IPv4, which the
/// configuration plane holds — the record `stado web route` writes is an A
/// record to exactly that address, so an answer that does not carry it is a
/// name pointing at something else.
///
/// For the Cloudflare edge it is `None`, and that is a statement rather than a
/// gap: a proxied record answers with Cloudflare's own anycast addresses,
/// which are theirs to change and not ours to enumerate, so asserting one
/// would produce a false finding every time they rotate. Such a product is
/// judged on whether the name resolves at all, which is the part that is
/// genuinely this fleet's business.
fn expected_address(declared: &WebApiProduct) -> Option<String> {
    if declared.edge() != "stado" {
        return None;
    }
    crate::config::web_api_edge()
        .ok()
        .map(|edge| edge.address().to_string())
}

/// One product's verdict and the row that explains it.
struct Verdict {
    row: Value,
    word: &'static str,
}

/// Everything one product's report needs, gathered from the four readers.
async fn examine(
    name: &str,
    declared: &WebApiProduct,
    managed: Option<&ServiceStatus>,
    runner: &Runner,
) -> Verdict {
    let unit = super::unit_label(name);
    let port = declared.port();
    let (dns_state, addresses) = resolve_hostname(declared.hostname()).await;
    let expected = expected_address(declared);

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
    let dns_detail = match (&expected, dns_state) {
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
        // A Cloudflare-edge product: the answer is reported and deliberately
        // not measured against an expectation.
        (None, _) => format!(
            "{} points at {} through the {} edge, whose addresses are not ours to assert",
            declared.hostname(),
            addresses.join(", "),
            declared.edge()
        ),
    };
    let dns_wrong = match (&expected, dns_state) {
        (_, DNS_UNREADABLE | DNS_UNRESOLVED) => true,
        (Some(address), _) => !addresses.iter().any(|found| found == address),
        (None, _) => false,
    };
    if word == VERDICT_SERVING && dns_wrong {
        word = VERDICT_DNS_ELSEWHERE;
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

/// The products this invocation reports on: one by name, or every declared
/// one.
///
/// An empty plane is not a broken one, exactly as `stado web list` treats it:
/// the plane's parser refuses an empty map so a half-written section cannot
/// pass, so "nothing declared" has to be recognised by the key being absent
/// rather than by the parse failing.
fn selected(
    name: Option<&str>,
) -> Result<Option<BTreeMap<String, &'static WebApiProduct>>, CmdError> {
    if let Some(name) = name {
        let declared = super::product(name)?;
        return Ok(Some(BTreeMap::from([(name.to_string(), declared)])));
    }
    match crate::config::web_api_products() {
        Ok(products) => Ok(Some(
            products
                .iter()
                .map(|(name, product)| (name.clone(), product))
                .collect(),
        )),
        Err(_) if crate::config_file::get("web_api.products").is_none() => Ok(None),
        Err(problems) => Err(CmdError::click(problems.join("; "))),
    }
}

pub(crate) async fn status(name: Option<&str>, json: bool) -> Result<(), CmdError> {
    let Some(products) = selected(name)? else {
        if json {
            println!("[]");
        } else {
            println!("no web products are declared");
        }
        return Ok(());
    };

    // One beacon read for every product, not one per product: the join is
    // fleet-wide already and asking again per row would pay a store listing
    // for each.
    let store = crate::cli::host::beacon_store().await?;
    let managed = service::list_services(&store).await.map_err(|error| {
        CmdError::click(format!(
            "the managed service set could not be read, so no unit state below could be judged: \
             {error}"
        ))
    })?;
    let runner = production_runner();

    let mut rows: Vec<Value> = Vec::with_capacity(products.len());
    let mut broken: Vec<String> = Vec::new();
    for (product, declared) in &products {
        let unit = super::unit_label(product);
        // Both spellings resolve, because both are how this unit is addressed:
        // the product's own name is what the registry records it under and the
        // label is what the host calls it.
        let row = managed
            .iter()
            .find(|row| row.service.matches(product) || row.service.matches(&unit));
        let verdict = examine(product, declared, row, &runner).await;
        if verdict.word != VERDICT_SERVING {
            broken.push(format!("{product}: {}", verdict.word));
        }
        rows.push(verdict.row);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            println!(
                "{} {} host={} port={} unit={} hostname={}",
                row["product"].as_str().unwrap_or_default(),
                row["verdict"].as_str().unwrap_or_default(),
                row["host"].as_str().unwrap_or_default(),
                row["port"].as_u64().unwrap_or_default(),
                row["unit"].as_str().unwrap_or_default(),
                row["hostname"].as_str().unwrap_or_default(),
            );
            println!(
                "  unit: {} (reported {})",
                row["unit_state"].as_str().unwrap_or("unknown"),
                match row["unit_reported_at"].as_str() {
                    Some(stamp) if !stamp.is_empty() => stamp,
                    _ => "never",
                }
            );
            println!(
                "  port: {}{}",
                row["port_state"].as_str().unwrap_or("unasked"),
                match row["port_detail"].as_str() {
                    Some(detail) if !detail.is_empty() => format!(" — {detail}"),
                    _ => String::new(),
                }
            );
            println!(
                "  dns:  {} — {}",
                row["dns_state"].as_str().unwrap_or("unknown"),
                row["dns_detail"].as_str().unwrap_or_default(),
            );
        }
    }

    if !broken.is_empty() {
        // Said on stderr so the table above stays parseable, and the command
        // exits non-zero on its own report rather than on a second reading of
        // it.
        eprintln!("not serving: {}", broken.join("; "));
        return Err(CmdError::silent(1));
    }
    Ok(())
}
