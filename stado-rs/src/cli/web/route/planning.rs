//! The DNS half read rather than written: the record `--check` says it would
//! write, what the hostname resolves to right now, how that set reads on one
//! line, and the zone a record lives in.

use serde_json::{json, Value};

use super::{CmdError, RECORD_TTL, RECORD_TYPE};
use crate::config::{WebApiEdge, WebApiProduct};

/// What the DNS step would change, read without writing and without the
/// registrar.
///
/// `--check` must write nothing at all, and `stado dns`'s merge is a
/// whole-zone read and a whole-zone write with no plan-only entry point that
/// does not need the credential. What decides the question anyway is the answer
/// the internet gives: a name that already resolves to exactly the edge's
/// address needs no record written, and one that resolves anywhere else does.
/// It is also the very fact `verify` depends on afterwards, so the check and
/// the verification are looking at the same thing.
pub(super) async fn planned_record(
    declared: &WebApiProduct,
    edge: &WebApiEdge,
) -> Result<Value, CmdError> {
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

pub(super) fn resolved_words(record: &Value) -> String {
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
pub(super) fn zone_of(hostname: &str) -> String {
    let labels: Vec<&str> = hostname.split('.').collect();
    if labels.len() >= 2 {
        labels[labels.len() - 2..].join(".")
    } else {
        hostname.to_string()
    }
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
    fn resolved_words_names_nothing_rather_than_printing_an_empty_list() {
        assert_eq!(resolved_words(&json!({ "resolves_to": [] })), "nothing");
        assert_eq!(
            resolved_words(&json!({ "resolves_to": ["76.76.21.21", "20.12.34.56"] })),
            "76.76.21.21, 20.12.34.56"
        );
    }
}
