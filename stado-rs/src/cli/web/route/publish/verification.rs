//! The third step: proving from outside that the hostname answers over TLS
//! from the fleet's own edge.

use serde_json::{json, Value};

use super::super::planning::zone_of;
use super::super::{CmdError, VERCEL_HEADER};
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
/// Asked once. Between the record moving and the certificate existing the
/// previous record's TTL has to expire and Let's Encrypt has to answer a
/// challenge; a hostname still inside that window is refused with the status,
/// `server` and `x-vercel-id` it answered with, and running `stado web route`
/// again asks again.
pub(super) async fn verify(declared: &WebApiProduct) -> Result<Value, CmdError> {
    let url = verify_url(declared);
    // The hint names the registrar credential the edge declares; an edge
    // that declares none leaves the placeholder, and one that is refused
    // says so inside it.
    let registrar = match crate::config::web_api_edge() {
        Ok(edge) => match edge.registrar_credential() {
            Some(item) => item.to_string(),
            None => "<registrar item>".to_string(),
        },
        Err(problems) => format!(
            "<registrar item; web_api.edge is refused: {}>",
            problems.join("; ")
        ),
    };
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = crate::wait::request(client.get(&url))
        .await
        .map_err(|error| {
            CmdError::unreachable(format!(
                "{url} could not be fetched: {error}. The record was written; read the zone with \
             `stado dns list {} --credential {}`.",
                zone_of(declared.hostname()),
                registrar,
            ))
        })?;
    let status = response.status();
    // A header is quoted as the answer carried it; one that is not visible
    // ASCII is quoted in its escaped form, and one absent is shown absent.
    let header = |name: &str| -> Option<String> {
        response
            .headers()
            .get(name)
            .map(|value| match value.to_str() {
                Ok(text) => text.to_string(),
                Err(_not_visible_ascii) => format!("{value:?}"),
            })
    };
    let server = header("server");
    let vercel = header(VERCEL_HEADER);
    let location = header("location");
    if status.is_success() && vercel.is_none() {
        return Ok(json!({
            "url": url,
            "status": status.as_u16(),
            "server": server,
            VERCEL_HEADER: Value::Null,
        }));
    }
    if let Some(vercel) = &vercel {
        return Err(CmdError::click(format!(
            "{url} answers from Vercel — HTTP {status}, server {server:?}, {VERCEL_HEADER} \
             {vercel:?} — so {} is not published by the fleet. The record was written; either the \
             previous record's TTL has not expired yet, or another record in the zone still points \
             at Vercel. Re-run this command, or read the zone with `stado dns list {} \
             --credential {}`.",
            declared.hostname(),
            zone_of(declared.hostname()),
            registrar,
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Err(CmdError::click(format!(
        "{url} answered HTTP {status}, server {server:?}, location {location:?}, not 2xx. The \
         record was written and the edge terminates the hostname, so the unit behind it is the \
         next thing to read: `stado web status {}`.",
        declared.hostname(),
    ))
    .stating(crate::primitives::failure::FailureCode::from_upstream_status(status.as_u16())))
}
