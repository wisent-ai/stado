//! Whether Cloudflare has published the name Cloudflare just handed out —
//! asked over HTTPS, because asking this machine's resolver is what breaks it.

use serde_json::Value;

/// Cloudflare's own DNS-over-HTTPS resolver, asked whether Cloudflare has
/// published the name Cloudflare just handed us.
///
/// This is not a control point and not a substitute address: it is the resolver
/// belonging to the service whose tunnel we are already running, asked one
/// question about that service's own zone. Nothing else in Stado is reached
/// through it.
const DOH_RESOLVER: &str = "https://cloudflare-dns.com/dns-query";

/// Ask whether the tunnel's hostname exists in DNS, **without ever asking the
/// operating system to resolve it.**
///
/// A `getaddrinfo` issued before Cloudflare publishes the record does not
/// merely fail — it leaves an `NXDOMAIN` in the local resolver's negative
/// cache, so every later attempt keeps failing from cache long after
/// Cloudflare has published the name. So the question goes to Cloudflare's
/// DoH endpoint over HTTPS instead. Only `cloudflare-dns.com` is resolved by
/// the operating system, and that name is not the one in danger.
///
/// The question is asked once. A name Cloudflare has not published is an
/// error naming the name, and a resolver that cannot be asked is an error
/// carrying the resolver's own answer.
pub async fn await_public_dns(host: &str) -> Result<(), String> {
    let response = crate::wait::request(
        reqwest::Client::new()
            .get(DOH_RESOLVER)
            .query(&[("name", host), ("type", "A")])
            .header("Accept", "application/dns-json"),
    )
    .await
    .map_err(|exc| format!("{DOH_RESOLVER} could not be asked about {host}: {exc}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "{DOH_RESOLVER} answered HTTP {status} when asked about {host}"
        ));
    }
    let document = response
        .json::<Value>()
        .await
        .map_err(|exc| format!("{DOH_RESOLVER} answered {host} with no JSON document: {exc}"))?;
    let published = document
        .get("Answer")
        .and_then(Value::as_array)
        .is_some_and(|answers| {
            answers
                .iter()
                .any(|answer| answer.get("data").and_then(Value::as_str).is_some())
        });
    if published {
        Ok(())
    } else {
        Err(format!(
            "Cloudflare has published no DNS record for {host} (its resolver answered {document}), \
             so the address it handed out does not exist yet and nothing could reach it"
        ))
    }
}
