//! The third step, and the only one that waits: proving from outside that the
//! hostname answers over TLS from the fleet's own edge.

use serde_json::{json, Value};

use super::super::planning::zone_of;
use super::super::{CmdError, VERCEL_HEADER, VERIFY_BUDGET, VERIFY_INTERVAL, VERIFY_TIMEOUT};
use super::verify_url;
use crate::config::WebApiProduct;

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
pub(super) async fn verify(declared: &WebApiProduct) -> Result<Value, CmdError> {
    let url = verify_url(declared);
    let client = reqwest::Client::builder()
        .timeout(VERIFY_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let started = tokio::time::Instant::now();
    let deadline = started + VERIFY_BUDGET;
    let mut observed = json!({
        "status":
            0,
        "server": "",
        VERCEL_HEADER: "",
        "detail": "not tried"
    });
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
                    "status":
                        0,
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
