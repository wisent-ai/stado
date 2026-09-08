//! Reserved addresses, whether or not anything is still using them.

use serde_json::{json, Value};

use crate::providers::gcp::inventory::fields::{aggregated, tail, text};

pub(in crate::providers::gcp::inventory) fn addresses_detail(
    value: &Value,
) -> (&'static str, Option<usize>, Value) {
    let addresses: Vec<Value> = aggregated(value, "addresses")
        .into_iter()
        .map(|address| {
            json!({
                "name": address.get("name"),
                "id": address.get("id"),
                "region": tail(text(address.get("region"))),
                "status": address.get("status"),
                "address_type": address.get("addressType"),
                "ip_version": address.get("ipVersion"),
                "purpose": address.get("purpose"),
                "users": address.get("users"),
                "creation_timestamp": address.get("creationTimestamp"),
                "self_link": address.get("selfLink"),
            })
        })
        .collect();
    let count = addresses.len();
    ("ok", Some(count), json!({"addresses": addresses}))
}
