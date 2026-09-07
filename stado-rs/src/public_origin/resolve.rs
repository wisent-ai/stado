//! Does a declared public origin's hostname exist for a client outside this
//! deployment?
//!
//! This machine is the worst possible witness. [`crate::tailnet`] exists
//! because a Stado origin in this fleet is a MagicDNS name and the local
//! resolver has to be told where the tailnet's names live; a workstation on
//! the tailnet therefore resolves `charless-mac-mini.tail6443b3.ts.net`
//! perfectly while a GitHub-hosted runner and a provider-managed edge get
//! nothing at all. A check run through the system resolver would have passed
//! on every machine an operator was likely to run it from, and failed on every
//! machine that matters.
//!
//! So the question is asked of a public resolver over DNS-over-HTTPS, and the
//! answer uses the three words `/docs/channels` already specifies for
//! `originDiagnosis`: `dns_unresolved` when the name has no public A or AAAA
//! record, `dns_resolved` when it has one and a failure is therefore the
//! connection or the handshake, and `dns_unavailable` when the resolver itself
//! could not be asked — which is never read as a missing name. The sentences
//! are word for word the ones the public release route returns, so the
//! operator at a terminal and the release gate reading a 503 body are reading
//! one sentence rather than two descriptions of one fact.
//!
//! The client is [`crate::cli::storage::fleet_https_client`], the one this
//! process already uses for every control-plane request. A second client would
//! be a second set of bounds to get wrong, and a resolver query that hangs
//! forever is the failure this whole module exists to name.

use serde::Deserialize;

/// A public resolver, deliberately not this machine's own: the question being
/// answered is what a client outside this deployment can resolve.
pub const PUBLIC_RESOLVER: &str = "https://cloudflare-dns.com/dns-query";

/// Address record types, by name. Both are asked before a name is called
/// unresolved, because an IPv6-only origin is published and reachable.
const ADDRESS_RECORD_TYPES: &[&str] = &["A", "AAAA"];

/// What a public resolver said about one hostname.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionState {
    /// The name has no public A or AAAA record.
    Unresolved,
    /// The name has at least one public address record.
    Resolved,
    /// The resolver could not be asked, so nothing is known.
    Unavailable,
}

impl ResolutionState {
    /// The word this state is reported as, matching `originDiagnosis.state`.
    pub fn word(self) -> &'static str {
        match self {
            Self::Unresolved => "dns_unresolved",
            Self::Resolved => "dns_resolved",
            Self::Unavailable => "dns_unavailable",
        }
    }
}

/// One public-resolution reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub state: ResolutionState,
    pub hostname: String,
    /// Every record found, as `"A 100.64.0.1"`, in query order.
    pub answers: Vec<String>,
    pub detail: String,
}

impl Resolution {
    /// The receipt field set every caller renders.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "state": self.state.word(),
            "resolver": PUBLIC_RESOLVER,
            "hostname": self.hostname,
            "answers": self.answers,
            "detail": self.detail,
        })
    }
}

#[derive(Deserialize)]
struct DnsAnswer {
    data: Option<String>,
}

#[derive(Deserialize)]
struct DnsResponse {
    #[serde(rename = "Answer")]
    answer: Option<Vec<DnsAnswer>>,
}

/// Ask a public resolver whether `hostname` has an address record.
pub async fn resolve(hostname: &str) -> Resolution {
    let client = match crate::cli::storage::fleet_https_client() {
        Ok(client) => client,
        Err(error) => {
            return unavailable(
                hostname,
                &format!(
                    "{hostname} could not be resolved because this Stado could not build its \
                     HTTPS client: {error}"
                ),
            )
        }
    };
    let mut answers = Vec::new();
    for kind in ADDRESS_RECORD_TYPES {
        let response = client
            .get(PUBLIC_RESOLVER)
            .header("accept", "application/dns-json")
            .query(&[("name", hostname), ("type", kind)])
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                return unavailable(
                    hostname,
                    &format!(
                        "{hostname} could not be resolved because the public resolver did not \
                         answer the {kind} query: {error}"
                    ),
                )
            }
        };
        if !response.status().is_success() {
            return unavailable(
                hostname,
                &format!(
                    "{hostname} could not be resolved because the public resolver answered HTTP \
                     {} to the {kind} query",
                    response.status().as_u16()
                ),
            );
        }
        let body = match response.json::<DnsResponse>().await {
            Ok(body) => body,
            Err(error) => {
                return unavailable(
                    hostname,
                    &format!(
                        "{hostname} could not be resolved because the public resolver's {kind} \
                         answer was not JSON: {error}"
                    ),
                )
            }
        };
        let Some(records) = body.answer else {
            continue;
        };
        for record in records {
            if let Some(data) = record.data {
                answers.push(format!("{kind} {data}"));
            }
        }
    }
    if answers.is_empty() {
        return Resolution {
            state: ResolutionState::Unresolved,
            hostname: hostname.to_string(),
            answers,
            detail: format!(
                "{hostname} has no public A or AAAA record, so nothing outside this deployment's \
                 own network can reach that origin, whatever it is serving"
            ),
        };
    }
    Resolution {
        state: ResolutionState::Resolved,
        hostname: hostname.to_string(),
        answers,
        detail: format!(
            "{hostname} resolves publicly, so a failure to read it is the connection or the TLS \
             handshake to a name that does exist"
        ),
    }
}

fn unavailable(hostname: &str, detail: &str) -> Resolution {
    Resolution {
        state: ResolutionState::Unavailable,
        hostname: hostname.to_string(),
        answers: Vec::new(),
        detail: detail.to_string(),
    }
}
