//! `stado web route` — putting one declared hostname on the public internet.
//!
//! Two things have to be true in the same place for `https://<hostname>` to
//! work: something the public internet can reach must answer on it, and that
//! something must hold a certificate for that exact name. This command makes
//! them true in that order, and the order is the whole design.
//!
//! **The edge is configured first, then the record moves, then the
//! certificate arrives.** Caddy cannot obtain a certificate for a name that
//! does not yet resolve to it: both HTTP-01 and TLS-ALPN-01 are challenges
//! Let's Encrypt delivers *to the edge, through DNS*. So the site block has to
//! exist before the record moves — otherwise the first request to arrive finds
//! a proxy that has never heard of the name — and the certificate can only be
//! issued after it. That makes the third step load-bearing rather than
//! cosmetic: the hostname is polled until it answers over TLS, and the elapsed
//! time is reported, because between the record and the certificate there is a
//! real window in which the name resolves to an edge that cannot yet complete
//! a handshake. Success is never reported on anything less than a completed
//! TLS request. `stado web remove` runs the reverse: the record goes first, so
//! nothing resolves to a hostname the edge is about to stop terminating.
//!
//! **Publication is verified from outside, and Vercel is the thing it looks
//! for.** `curl -sI https://preferences.wisent.com/` answers 200 with
//! `server: Vercel` and an `x-vercel-id` header today, and the fleet is
//! already serving those bytes — Vercel contributes exactly one thing, a
//! certificate for a `wisent.com` name. So a 200 alone proves nothing: it is
//! the same 200 the hostname returned before this command ran. The check is a
//! 2xx **and** the absence of `x-vercel-id`, and a hostname still answering
//! from Vercel is reported as unpublished with the `server` and `x-vercel-id`
//! values that were actually observed. Nothing here removes a Vercel project;
//! a hostname stops being served by Vercel when its record stops pointing
//! there, and that record is this command's last step.
//!
//! **A zone at Cloudflare takes the other edge, and cannot be exercised
//! today.** [`crate::cli::cloudflare`] already speaks tunnel routing, and the
//! credential it needs does not exist in Skarbiec — so that arm refuses with
//! that fact rather than half-working or quietly falling back to the Stado
//! edge, which would publish the name from an edge the operator did not
//! choose.

use std::time::Duration;

use serde_json::{json, Value};

use super::CmdError;
use crate::config::{WebApiEdge, WebApiProduct};

/// The record every `stado`-edge hostname gets: an A record at the edge's own
/// public address.
const RECORD_TYPE: &str = "A";

/// Half an hour. Long enough that the registrar is not asked about a
/// production name on every request, short enough that moving the edge is a
/// half-hour cutover rather than a day.
const RECORD_TTL: &str = "1800";

/// The Skarbiec item holding the registrar's `api_user`, `api_key`, `username`
/// and `client_ip`. The same default `stado dns` uses, because a product's
/// hostname and an operator's hand-typed record must take one path through the
/// registrar.
const REGISTRAR_CREDENTIAL: &str = "namecheap_auto";

/// The header a Vercel edge stamps on every response it serves. Its presence
/// is the one unambiguous proof that a hostname has not moved to the fleet.
const VERCEL_HEADER: &str = "x-vercel-id";

/// How long the hostname is given to answer over TLS from the edge.
///
/// Three things happen inside this window, in order: the previous record's
/// TTL expires, the new record propagates, and Let's Encrypt answers a
/// challenge it delivers to the edge over the record that just moved. The
/// record this command writes carries [`RECORD_TTL`], but the one that governs
/// the wait is whatever TTL the *previous* record carried, and a first-time
/// issuance adds its own. Five minutes covers all three; beyond that something
/// is wrong and saying so beats waiting.
const VERIFY_BUDGET: Duration = Duration::from_secs(300);

/// Gap between verification attempts.
const VERIFY_INTERVAL: Duration = Duration::from_secs(5);

/// Per-request ceiling for one verification attempt.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) async fn route(name: &str, check: bool, json: bool) -> Result<(), CmdError> {
    let declared = super::product(name)?;
    match declared.edge() {
        "stado" => publish(name, declared, check, json).await,
        "cloudflare" => Err(CmdError::click(cloudflare_unavailable(declared.hostname()))),
        other => Err(CmdError::click(format!(
            "web product {name} declares edge {other:?}, and no publication path implements it"
        ))),
    }
}

/// Drop the hostname's record and stop the edge terminating it.
///
/// The record first, deliberately: after it is gone nothing resolves to the
/// edge for this name, so removing the site block cannot strand a live
/// request. `stado web remove` calls this while the product is still declared
/// — the declaration is only forgotten once the unit is retired — which is why
/// the desired route set handed to the edge is computed here by excluding this
/// hostname rather than by re-reading the declarations.
pub(crate) async fn retract(name: &str, declared: &WebApiProduct) -> Result<Value, CmdError> {
    match declared.edge() {
        "cloudflare" => Ok(json!({
            "hostname": declared.hostname(),
            "change": "unchanged",
            "refused": cloudflare_unavailable(declared.hostname()),
        })),
        "stado" => {
            // A mount does not own the hostname, so retracting it must not
            // touch the record: the owner still answers there and every other
            // mount under it still does too. What comes out is one
            // `handle_path` block, and nothing else.
            if let Some(prefix) = declared.path_prefix() {
                let edge = match crate::config::web_api_edge() {
                    Ok(edge) => edge,
                    Err(_) => {
                        return Ok(json!({
                            "hostname": declared.hostname(),
                            "path_prefix": prefix,
                            "change": "unchanged",
                            "record": Value::Null,
                            "edge": Value::Null,
                        }))
                    }
                };
                let mounted = super::edge::mount(
                    declared.hostname(),
                    prefix,
                    declared.host(),
                    declared.port(),
                )?;
                let routes: Vec<(String, Vec<String>)> = super::edge::stado_routes()
                    .await?
                    .into_iter()
                    .map(|(hostname, mut block)| {
                        if hostname == declared.hostname() {
                            block.retain(|directive| directive != &mounted.1);
                        }
                        (hostname, block)
                    })
                    .collect();
                let edge_report = super::edge::deliver(edge, &routes, true).await?;
                return Ok(json!({
                    "hostname": declared.hostname(),
                    "path_prefix": prefix,
                    "change": "removed",
                    // The record belongs to whichever declaration owns this
                    // hostname, and it is still published there.
                    "record": Value::Null,
                    "edge": edge_report,
                }));
            }
            let record = crate::cli::dns::remove_record(
                declared.hostname(),
                RECORD_TYPE,
                None,
                REGISTRAR_CREDENTIAL,
            )
            .await?;
            let removed = record["removed"].as_u64().unwrap_or_default() > 0;
            // An undeclared edge is not a failed retraction: the record is
            // already gone, so the hostname is unpublished, and there is no
            // proxy configuration for it to still appear in.
            let edge = match crate::config::web_api_edge() {
                Ok(edge) => edge,
                Err(_) => {
                    return Ok(json!({
                        "hostname": declared.hostname(),
                        "change": if removed { "removed" } else { "unchanged" },
                        "record": record,
                        "edge": Value::Null,
                    }))
                }
            };
            let routes: Vec<(String, Vec<String>)> = super::edge::stado_routes()
                .await?
                .into_iter()
                .filter(|(hostname, _)| hostname != declared.hostname())
                .collect();
            let edge_report = super::edge::deliver(edge, &routes, true).await?;
            Ok(json!({
                "hostname": declared.hostname(),
                "change": if removed { "removed" } else { "unchanged" },
                "record": record,
                "edge": edge_report,
            }))
        }
        other => Err(CmdError::click(format!(
            "web product {name} declares edge {other:?}, and no retraction path implements it"
        ))),
    }
}

/// The URL a product's publication is proved by.
///
/// A mount is proved at its own prefix: the owner's `readyz` is a path on the
/// owner's application and says nothing about whether `/docs` reaches this
/// unit. The trailing slash is deliberate — `/docs` and `/docs/` both match
/// the mount's matcher, and the slash is what the mount's own root resolves
/// to.
fn verify_url(declared: &WebApiProduct) -> String {
    match declared.path_prefix() {
        Some(prefix) => format!("https://{}{prefix}/", declared.hostname()),
        None => format!("https://{}{}", declared.hostname(), declared.readyz()),
    }
}

async fn publish(
    name: &str,
    declared: &WebApiProduct,
    check: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let edge = super::edge::declared()?;
    let routes = super::edge::stado_routes().await?;
    // The edge first, always. `check` false is what makes this the write; with
    // `check` true nothing is delivered and nothing is written locally either.
    let edge_report = super::edge::deliver(edge, &routes, !check).await?;

    if check {
        let record = planned_record(declared, edge).await?;
        let settled = edge_report["change"].as_str() == Some("unchanged")
            && record["change"].as_str() == Some("unchanged");
        let report = json!({
            "product": name,
            "hostname": declared.hostname(),
            "edge": edge_report,
            "record": record,
            "change": if settled { "unchanged" } else { "would-change" },
        });
        if json_output {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!(
                "{}: edge {}, record {} ({} -> {})",
                declared.hostname(),
                edge_report["change"].as_str().unwrap_or_default(),
                record["change"].as_str().unwrap_or_default(),
                resolved_words(&record),
                edge.address(),
            );
        }
        // Nothing was written, so the only useful answer to "is this already
        // published" is the exit code.
        return if settled {
            Ok(())
        } else {
            Err(CmdError::silent(1))
        };
    }

    // A mount writes no record. The hostname's A record belongs to the
    // declaration that owns it and already points at this edge; writing it
    // again from here would be a second writer of one name, and removing this
    // mount would then look like it should take the record with it.
    let record = if declared.path_prefix().is_some() {
        json!({
            "change": "unchanged",
            "detail": format!(
                "{} is owned by another declaration, whose record already points at this edge",
                declared.hostname()
            ),
        })
    } else {
        crate::cli::dns::ensure_record(
            declared.hostname(),
            RECORD_TYPE,
            edge.address(),
            RECORD_TTL,
            None,
            REGISTRAR_CREDENTIAL,
        )
        .await?
    };
    let served = verify(declared).await?;
    let mut report = json!({
        "product": name,
        "hostname": declared.hostname(),
        "edge": edge_report,
        "record": record,
        "served": served,
        "change": "published",
    });
    if let Some(prefix) = declared.path_prefix() {
        report["path_prefix"] = json!(prefix);
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} answers {} from {} (edge {}, record {})",
            verify_url(declared),
            served["status"].as_u64().unwrap_or_default(),
            served["server"].as_str().unwrap_or("an unnamed server"),
            report["edge"]["change"].as_str().unwrap_or_default(),
            report["record"]["change"].as_str().unwrap_or_default(),
        );
    }
    Ok(())
}

/// What the DNS step would change, read without writing and without the
/// registrar.
///
/// `--check` must write nothing at all, and `stado dns`'s merge is a
/// whole-zone read and a whole-zone write with no plan-only entry point that
/// does not need the credential. What decides the question anyway is the answer
/// the internet gives: a name that already resolves to exactly the edge's
/// address needs no record written, and one that resolves anywhere else does.
/// It is also the very fact [`verify`] depends on afterwards, so the check and
/// the verification are looking at the same thing.
async fn planned_record(declared: &WebApiProduct, edge: &WebApiEdge) -> Result<Value, CmdError> {
    let resolved = resolve(declared.hostname()).await;
    let settled = resolved.len() == 1 && resolved[0] == edge.address();
    Ok(json!({
        "name": declared.hostname(),
        "zone": zone_of(declared.hostname()),
        "type": RECORD_TYPE,
        "value": edge.address(),
        "ttl": RECORD_TTL,
        "resolves_to": resolved,
        "change": if settled { "unchanged" } else { "would-write" },
    }))
}

/// Every IPv4 address this machine's resolver returns for the hostname.
///
/// IPv4 only, because the record under discussion is an A record; an AAAA
/// record the zone happens to carry says nothing about whether this one is
/// right. A name that resolves to nothing is an empty list rather than an
/// error: "no record at all" is a perfectly ordinary state for a hostname
/// about to be published for the first time.
async fn resolve(hostname: &str) -> Vec<String> {
    let Ok(addresses) = tokio::net::lookup_host((hostname, 443u16)).await else {
        return Vec::new();
    };
    let mut resolved: Vec<String> = addresses
        .filter(|address| address.is_ipv4())
        .map(|address| address.ip().to_string())
        .collect();
    resolved.sort();
    resolved.dedup();
    resolved
}

fn resolved_words(record: &Value) -> String {
    match record["resolves_to"].as_array() {
        Some(addresses) if !addresses.is_empty() => addresses
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        _ => "nothing".to_string(),
    }
}

/// The zone a hostname's record lives in: its last two labels, which is what
/// `stado dns` itself defaults to when no zone is named.
fn zone_of(hostname: &str) -> String {
    let labels: Vec<&str> = hostname.split('.').collect();
    if labels.len() >= 2 {
        labels[labels.len() - 2..].join(".")
    } else {
        hostname.to_string()
    }
}

/// Prove the hostname is answering from the fleet's own edge.
///
/// A 2xx and no `x-vercel-id`. Both halves are load-bearing: the hostname
/// answered 2xx before this command ran, from Vercel, so a status check alone
/// would report success for a name that never moved. Redirects are not
/// followed, because a 2xx reached through someone else's redirect is not this
/// hostname answering — and the location that was returned instead is named in
/// the failure so the redirect is diagnosable.
///
/// Retried rather than checked once, and the elapsed time is part of the
/// report: between the record moving and the certificate existing there is a
/// real window — the previous record's TTL has to expire and Let's Encrypt has
/// to answer a challenge it delivers to the edge — and how long that took is
/// the number an operator needs when the next hostname is cut over. What is
/// never retried away is the finding: when the budget is spent the refusal
/// carries the last observed status, `server` and `x-vercel-id`.
async fn verify(declared: &WebApiProduct) -> Result<Value, CmdError> {
    let url = verify_url(declared);
    let client = reqwest::Client::builder()
        .timeout(VERIFY_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let started = tokio::time::Instant::now();
    let deadline = started + VERIFY_BUDGET;
    let mut observed =
        json!({ "status": 0, "server": "", VERCEL_HEADER: "", "detail": "not tried" });
    loop {
        match client.get(&url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let header = |name: &str| {
                    response
                        .headers()
                        .get(name)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string()
                };
                let server = header("server");
                let vercel = header(VERCEL_HEADER);
                let location = header("location");
                if (200..300).contains(&status) && vercel.is_empty() {
                    return Ok(json!({
                        "url": url,
                        "status": status,
                        "server": server,
                        VERCEL_HEADER: Value::Null,
                        "elapsed_seconds": started.elapsed().as_secs(),
                    }));
                }
                observed = json!({
                    "status": status,
                    "server": server,
                    VERCEL_HEADER: vercel,
                    "detail": location,
                });
            }
            Err(error) => {
                observed = json!({
                    "status": 0,
                    "server": "",
                    VERCEL_HEADER: "",
                    "detail": error.to_string(),
                });
            }
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(VERIFY_INTERVAL).await;
    }

    let status = observed["status"].as_u64().unwrap_or_default();
    let server = observed["server"].as_str().unwrap_or_default();
    let vercel = observed[VERCEL_HEADER].as_str().unwrap_or_default();
    let detail = observed["detail"].as_str().unwrap_or_default();
    if !vercel.is_empty() {
        return Err(CmdError::click(format!(
            "{url} still answers from Vercel after {}s — HTTP {status}, server {server:?}, \
             {VERCEL_HEADER} {vercel:?} — so {} is not published by the fleet. The record was \
             written; either the previous record's TTL has not expired yet, or another record in \
             the zone still points at Vercel. Re-run this command, or read the zone with \
             `stado dns list {}`.",
            VERIFY_BUDGET.as_secs(),
            declared.hostname(),
            zone_of(declared.hostname()),
        )));
    }
    Err(CmdError::click(format!(
        "{url} did not answer 2xx within {}s: HTTP {status}, server {server:?} ({detail}). The \
         record was written and the edge terminates the hostname, so the unit behind it is the \
         next thing to read: `stado web status {}`.",
        VERIFY_BUDGET.as_secs(),
        declared.hostname(),
    )))
}

/// Why the Cloudflare edge cannot be exercised, in the vault's own terms.
///
/// Measured, not assumed: `platform-admin-cloudflare` carries `username` and
/// `password` — a console login. `platform-cloudflare-bobloo-tunnel` carries
/// `account_id`, `token`, `tunnel_id` and `tunnel_name`, and its `token` is a
/// 180-character `cloudflared` tunnel token which the Cloudflare API rejects
/// as a bearer with code 6111, `Invalid format for Authorization header`.
/// [`crate::cli::cloudflare`] requires an `--api-credential` item carrying
/// `account_id` and `api_token`, and no item in Skarbiec carries an
/// `api_token` at all.
///
/// This refusal is the whole arm on purpose. Falling back to the Stado edge
/// would publish the hostname from an edge the declaration did not choose, and
/// inventing a credential is not something a control plane does.
fn cloudflare_unavailable(hostname: &str) -> String {
    let zone = zone_of(hostname);
    format!(
        "{hostname} declares the cloudflare edge, and Skarbiec holds no item carrying the \
         `api_token` field that `stado cloudflare --api-credential` requires: \
         `platform-admin-cloudflare` carries only a console `username` and `password`, and \
         `platform-cloudflare-bobloo-tunnel` carries `account_id`, `token`, `tunnel_id` and \
         `tunnel_name` — its `token` is a cloudflared tunnel token, which the Cloudflare API \
         refuses as a bearer with code 6111, `Invalid format for Authorization header`. Add a \
         Skarbiec item carrying `account_id` and a scoped `api_token`, and \
         `stado cloudflare route-tunnel --api-credential <that item> --tunnel-credential \
         platform-cloudflare-bobloo-tunnel --zone {zone} --hostname {hostname}` publishes it. \
         {zone} must also be a zone Cloudflare's nameservers serve, because Cloudflare issues \
         that certificate only for a zone it serves; a zone at Namecheap has to declare \
         `--edge stado` instead."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zone_is_the_last_two_labels_the_way_stado_dns_defaults() {
        assert_eq!(zone_of("app.preferences.wisent.com"), "wisent.com");
        assert_eq!(zone_of("wisent.com"), "wisent.com");
        assert_eq!(zone_of("localhost"), "localhost");
    }

    #[test]
    fn the_cloudflare_refusal_names_the_field_the_item_and_the_zone_requirement() {
        let refusal = cloudflare_unavailable("bobloo.bobloo.com");
        // The three things the operator has to be told, and the two items that
        // were actually read out of the vault.
        assert!(refusal.contains("`api_token`"), "{refusal}");
        assert!(refusal.contains("--api-credential"), "{refusal}");
        assert!(
            refusal.contains("platform-cloudflare-bobloo-tunnel"),
            "{refusal}"
        );
        assert!(refusal.contains("platform-admin-cloudflare"), "{refusal}");
        assert!(
            refusal.contains("bobloo.com must also be a zone Cloudflare's nameservers serve"),
            "{refusal}"
        );
        // And no suggestion that the stado edge will quietly do it instead.
        assert!(!refusal.contains("falling back"), "{refusal}");
    }

    #[test]
    fn resolved_words_names_nothing_rather_than_printing_an_empty_list() {
        assert_eq!(resolved_words(&json!({ "resolves_to": [] })), "nothing");
        assert_eq!(
            resolved_words(&json!({ "resolves_to": ["76.76.21.21", "20.12.34.56"] })),
            "76.76.21.21, 20.12.34.56"
        );
    }
}
