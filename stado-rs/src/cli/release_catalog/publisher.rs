//! `stado release catalog declare-publisher`: everything a product needs
//! before `release submit` can publish it, in the order the guards require,
//! from the typed operations this fleet already has.
//!
//! The command mints the product's publisher item, grants Stado read access,
//! declares it on the participating hosts and checks their release policies.

use serde_json::{json, Value};

use crate::cli::host::{
    grant_item_read, store_vault_item, vault_item_state, vault_token_sync, vault_word,
    write_host_config,
};
use crate::cli::CmdError;

/// Bytes of randomness in a minted publisher bearer; the same width the
/// verifier reconciliation mints (`openssl rand -hex 32`).
const BEARER_BYTES: usize = 32;

/// The publisher item and prefix a product's declaration names.
pub(super) fn publisher_declaration(product: &str) -> (String, Value) {
    let item = product.to_string();
    let declared = json!({ "item": item, "prefix": format!("{product}/") });
    (item, declared)
}

/// A fresh bearer: two random UUIDs' bytes, hex encoded, so no shell and no
/// argument vector ever carries it.
fn mint_bearer() -> String {
    let mut bytes = Vec::with_capacity(BEARER_BYTES);
    while bytes.len() < BEARER_BYTES {
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    bytes.truncate(BEARER_BYTES);
    hex::encode(bytes)
}

/// `~/…` for a path under this machine's home, unchanged otherwise.
fn home_relative(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    }
}

/// Declare `product`'s release publisher across the fleet.
///
/// `owner` holds the authoritative vault, `client` submits the release,
/// `targets` serve the release API and `reloads` names the managed services
/// whose publisher policy must be refreshed after the declaration changes.
pub(super) async fn declare_publisher(
    product: &str,
    owner: &str,
    client: &str,
    targets: &[String],
    reloads: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    let reloads = reloads
        .iter()
        .map(|pair| {
            pair.split_once('=')
                .map(|(host, service)| (host.to_string(), service.to_string()))
                .ok_or_else(|| {
                    CmdError::usage(format!("--reload takes HOST=SERVICE, not {pair:?}"))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    vault_word("product", product)?;
    let (item, declared) = publisher_declaration(product);
    let consumer = crate::config::skarbiec_consumer().to_string();
    let token_file = home_relative(crate::config::skarbiec_token_file());
    let mut report = Vec::new();

    // 1. The item, on the owner, once.
    let state = vault_item_state(owner, &item).await?;
    let minted = state == "absent";
    if minted {
        // The canonical item envelope Skarbiec's `set-json` accepts: schema,
        // kind, the one required field, and the context that names the
        // product the bearer publishes.
        let payload = json!({
            "schema": "skarbiec.item.v2",
            "kind": "token",
            "fields": { "token": mint_bearer() },
            "context": { "product": product, "role": "release-publisher" },
        })
        .to_string();
        store_vault_item(owner, &item, "token", &payload, false).await?;
    }
    report.push(json!({ "step": "item", "host": owner, "item": item, "minted": minted, "state_before": state }));

    // 2. The release client's bearer beside the owner's vault, so its grant
    //    can be widened there and not on a replica the owner overwrites.
    if client != owner {
        vault_token_sync(
            client,
            owner,
            &consumer,
            &token_file,
            &token_file,
            false,
            false,
        )
        .await?;
        report
            .push(json!({ "step": "bearer", "from": client, "host": owner, "consumer": consumer }));
    }
    grant_item_read(owner, &consumer, &item, "token", &token_file, false).await?;
    report.push(json!({ "step": "grant", "host": owner, "consumer": consumer, "capability": format!("read:{item}#token") }));

    // 3. The declaration, on every host that serves or submits releases. The
    //    write is guarded: a host that does not hold the item refuses it.
    let key = format!("release_api.publishers.{product}");
    let value = declared.to_string();
    let mut declared_on = vec![owner.to_string(), client.to_string()];
    declared_on.extend(targets.iter().cloned());
    declared_on.sort();
    declared_on.dedup();
    for host in &declared_on {
        write_host_config(host, &key, &value).await?;
        report.push(json!({ "step": "declare", "host": host, "key": key, "value": declared }));
    }

    // 4. The verifier's grant, reconciled to the declaration by the product's
    //    own repair step - in a fresh process, because this one read the
    //    configuration before the declaration above was written to it, and
    //    before any unit is reconciled: the repair reaches the owner through
    //    the local data plane, which a reload takes down for a moment.
    let repair = std::process::Command::new(std::env::current_exe()?)
        .args([
            "repair",
            "stado",
            "--step",
            "release-verifier",
            "--target",
            owner,
            "--apply",
        ])
        .output()?;
    if !repair.status.success() {
        return Err(CmdError::click(format!(
            "release-verifier repair on {owner} failed after the declaration was written: {}",
            String::from_utf8_lossy(&repair.stderr).trim()
        )));
    }
    report.push(json!({ "step": "verifier", "host": owner, "repair": "release-verifier" }));

    // 5. The units whose processes cache the publisher table, last.
    for (host, service) in &reloads {
        crate::cli::service::reconcile_after_config_change(service, host).await?;
        report.push(json!({ "step": "reload", "host": host, "service": service }));
    }

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({ "product": product, "item": item, "steps": report })
            )?
        );
    } else {
        println!(
            "{product}: publisher {item} {} on {owner}; {consumer} may read it; declared on {}; release verifier reconciled",
            if minted { "minted" } else { "already held" },
            declared_on.join(", ")
        );
    }
    Ok(())
}
