//! Whether Cloudflare has published the name Cloudflare just handed out —
//! asked over HTTPS, because asking this machine's resolver is what breaks it.

use serde_json::Value;

use crate::cli::fleet::ingress::{DNS_DEADLINE, DNS_POLL, FETCH_TIMEOUT};

/// Cloudflare's own DNS-over-HTTPS resolver, asked whether Cloudflare has
/// published the name Cloudflare just handed us.
///
/// This is not a control point and not a substitute address: it is the resolver
/// belonging to the service whose tunnel we are already running, asked one
/// question about that service's own zone. Nothing else in Stado is reached
/// through it.
const DOH_RESOLVER: &str = "https://cloudflare-dns.com/dns-query";

/// Wait until the tunnel's hostname exists in DNS, **without ever asking the
/// operating system to resolve it.**
///
/// This is not caution, it is the difference between working and not. The
/// record appears roughly six seconds after `cloudflared` prints the address,
/// and a `getaddrinfo` issued in that window does not merely fail — it leaves
/// an `NXDOMAIN` in the local resolver's negative cache, so every later attempt
/// keeps failing from cache long after Cloudflare has published the name.
/// Measured here: a lookup at second zero made the address unresolvable for the
/// next 64 seconds while `trycloudflare.com`'s own resolver had been answering
/// since second six. Wherever the zone's negative TTL is honoured rather than
/// clamped, that is 1800 seconds. One premature question costs the whole
/// entrance.
///
/// So the question goes to Cloudflare's DoH endpoint over HTTPS instead. Only
/// `cloudflare-dns.com` is resolved by the operating system, and that name is
/// not the one in danger.
///
/// A resolver that cannot be reached at all is **not** a failure: this step
/// exists to protect the local cache, not to decide anything. It returns and
/// lets the fetch that follows be the thing that decides — with the original
/// risk, and no worse than not having asked.
pub async fn await_public_dns(host: &str) -> Result<(), String> {
    let client = match reqwest::Client::builder().timeout(FETCH_TIMEOUT).build() {
        Ok(client) => client,
        Err(_) => return Ok(()),
    };
    let deadline = tokio::time::Instant::now() + DNS_DEADLINE;
    let mut resolver_answered = false;
    loop {
        let response = client
            .get(DOH_RESOLVER)
            .query(&[("name", host), ("type", "A")])
            .header("Accept", "application/dns-json")
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                resolver_answered = true;
                if let Ok(document) = response.json::<Value>().await {
                    let published = document
                        .get("Answer")
                        .and_then(Value::as_array)
                        .is_some_and(|answers| {
                            answers
                                .iter()
                                .any(|answer| answer.get("data").and_then(Value::as_str).is_some())
                        });
                    if published {
                        return Ok(());
                    }
                }
            }
            // The resolver is unreachable or unhappy. Not our verdict to make.
            _ if !resolver_answered => return Ok(()),
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "Cloudflare published no DNS record for {host} within {}s, so the address it just \
                 handed out does not exist yet and nothing could reach it",
                DNS_DEADLINE.as_secs()
            ));
        }
        tokio::time::sleep(DNS_POLL).await;
    }
}
