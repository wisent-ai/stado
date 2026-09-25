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
pub(super) fn home_relative(path: &str) -> String {
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

    // 4. Reconcile every host that accepted the declaration. Each verifier
    //    compares its grant with its own publisher table; repairing only the
    //    owner leaves the client and API targets failing closed. Use a fresh
    //    process because this one read configuration before the writes above.
    for host in &declared_on {
        let repair = std::process::Command::new(std::env::current_exe()?)
            .args([
                "repair",
                "stado",
                "--step",
                "release-verifier",
                "--target",
                host,
                "--apply",
            ])
            .output()?;
        if !repair.status.success() {
            return Err(CmdError::click(format!(
                "release-verifier repair on {host} failed after the declaration was written: {}",
                String::from_utf8_lossy(&repair.stderr).trim()
            )));
        }
        report.push(json!({ "step": "verifier", "host": host, "repair": "release-verifier" }));
    }

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

/// Declare `product`'s publisher when this host's configuration does not,
/// so the first build or release of a new product publishes instead of
/// refusing with `release_api.publishers declares no publisher`.
///
/// That refusal used to be the only way a new product learned it needed
/// `catalog declare-publisher`, and the command needed a person to know which
/// host owns the vault. Both hosts are facts Stado can read: the client is
/// this host's registry target, and the owner is the host this vault
/// replicates, or this host when its vault is the authority. The declaration
/// itself is `declare_publisher`, unchanged.
pub(super) async fn ensure_publisher(product: &str) -> Result<(), CmdError> {
    if crate::config::release_publisher_declared(product) {
        return Ok(());
    }
    // A publisher is the bearer a write to the release object API carries.
    // A store this process writes directly — the local backend with no API
    // configured, as an isolated release journey runs — takes no bearer, and
    // asking which host owns the fleet vault there refused every such run
    // with `no installed Skarbiec launcher` before its first write.
    if crate::config::stado_api_url().is_empty()
        && crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
            == Some(crate::capabilities::StorageAdapter::Local)
    {
        return Ok(());
    }
    let (owner, client) = fleet_hosts().await?;
    eprintln!(
        "{product}: this host declares no release publisher for it; declaring it now \
         (vault owner {owner}, release client {client})"
    );
    declare_publisher(product, &owner, &client, &[], &[], false)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "{product} has no release publisher and declaring one failed \
                 (stado release catalog declare-publisher {product} --owner {owner} \
                 --client {client}): {error}"
            ))
        })
}

/// The vault owner and this host, as registry target names.
pub(super) async fn fleet_hosts() -> Result<(String, String), CmdError> {
    let client = this_host().await?;
    let owner = vault_owner(&client)?;
    Ok((owner, client))
}

/// This host's registry target, as `stado resolver` identifies it.
pub(crate) async fn this_host() -> Result<String, CmdError> {
    let store = std::sync::Arc::new(crate::targets::RegistryStore::open().await?);
    let (bootstrap, _, _) = crate::cli::resolver::read_local_snapshot(&store)
        .await
        .map_err(CmdError::click)?;
    crate::cli::resolver::current_target(&bootstrap).map_err(CmdError::click)
}

/// The host that owns the fleet vault: the bond this host's vault
/// replicates, or this host when its vault replicates nothing. A vault whose
/// status cannot be read is refused rather than guessed, because a publisher
/// minted on a replica is overwritten by the next pull.
fn vault_owner(this_host: &str) -> Result<String, CmdError> {
    let declared = crate::config::skarbiec_vault_file();
    let vault = if declared.is_empty() {
        let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
        std::path::Path::new(&home).join(".stado/skarbiec.vault.json")
    } else {
        std::path::PathBuf::from(declared)
    };
    let launcher = crate::cli::secrets::skarbiec_launcher()?;
    let status = crate::cli::secrets::launcher_json(&launcher, &vault, &["sync-status"]).map_err(
        |error| {
            CmdError::click(format!(
                "cannot tell which host owns the vault: skarbiec sync-status on {} failed: {error}",
                vault.display()
            ))
        },
    )?;
    let bonds = status.as_array().ok_or_else(|| {
        CmdError::click(format!(
            "cannot tell which host owns the vault: skarbiec sync-status on {} answered {status}",
            vault.display()
        ))
    })?;
    Ok(bonds
        .iter()
        .find(|bond| bond.get("role").and_then(Value::as_str) == Some("replica"))
        .and_then(|bond| bond.get("bond").and_then(Value::as_str))
        .map_or_else(|| this_host.to_owned(), str::to_owned))
}
