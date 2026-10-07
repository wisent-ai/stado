//! `stado release catalog declare-publisher`: everything a product needs
//! before `release submit` can publish it, in the order the guards require,
//! from the typed operations this fleet already has.
//!
//! The command stores the product's publisher bearer in the item named after
//! the product, which also plays the role of that name, grants Stado read
//! access, declares it on the participating hosts and checks their release
//! policies.

use serde_json::{json, Value};

use crate::cli::host::{
    grant_item_read, named_role_items_out_of_shape, vault_token_sync, vault_word,
    write_host_config, write_named_role_item, TokenSyncMode,
};
use crate::cli::CmdError;

mod hosts;
mod input;
mod reload;
mod withdraw;

pub(crate) use hosts::{fleet_hosts, this_host};
pub(super) use input::pin_input;
pub(super) use withdraw::withdraw_publisher;

/// Bytes of randomness in a minted publisher bearer; the same width the
/// verifier reconciliation mints (`openssl rand -hex 32`).
const BEARER_BYTES: usize = 32;

/// The publisher item is the product itself; its scope is `<product>/`.
/// This matches the strict release publisher configuration contract.
pub(super) fn publisher_declaration(product: &str) -> (String, Value) {
    let role = product.to_owned();
    let declared = json!({ "item": role, "prefix": format!("{product}/") });
    (role, declared)
}

/// A fresh bearer: two random UUIDs' bytes, hex encoded, so no shell and no
/// argument vector ever carries it. `stado credentials token mint
/// --store-item` writes one into its item the same way.
pub(crate) fn mint_bearer() -> String {
    let mut bytes = Vec::with_capacity(BEARER_BYTES);
    while bytes.len() < BEARER_BYTES {
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    bytes.truncate(BEARER_BYTES);
    hex::encode(bytes)
}

/// `~/…` for a path under this machine's home, unchanged otherwise.
pub(crate) fn home_relative(path: &str) -> String {
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
    let reloads = reload::reload_targets(reloads)?;
    vault_word("product", product)?;
    let (role, declared) = publisher_declaration(product);
    let consumer = crate::config::skarbiec_consumer().to_string();
    let token_file = home_relative(crate::config::skarbiec_token_file());
    let mut report = Vec::new();

    // 1. The bearer, on the owner, once, in the item named after the product
    //    and playing its role: every reader of the declaration (the release
    //    verifier, a build staging an input, enrolment's check) asks for that
    //    item by name, and the guard on step 3 refuses a declaration whose
    //    item the owner does not hold. The canonical item envelope Skarbiec's
    //    `set-json` accepts: schema, kind, the one required field, and the
    //    context that names the product the bearer publishes.
    let payload = json!({
        "schema": "skarbiec.item.v2",
        "kind": "token",
        "fields": { "token": mint_bearer() },
        "context": { "product": product, "role": "release-publisher" },
    })
    .to_string();
    let stored = write_named_role_item(owner, &role, &role, "token", &payload).await?;
    let minted = stored["created"].as_bool() != Some(false);
    report.push(json!({
        "step": "item", "host": owner, "role": role, "minted": minted, "stored": stored,
    }));

    // 2. The release client's bearer beside the owner's vault, so its grant
    //    can be widened there and not on a replica the owner overwrites. A
    //    client that holds no vault of its own reads the owner's through
    //    secrets.skarbiec.url, so its bearer already lives there and copying
    //    it would ask for a local vault authority it does not have.
    let client_reads_owner = this_host().await.is_ok_and(|here| here == client)
        && crate::config::skarbiec_vault_file().trim().is_empty();
    if client != owner && !client_reads_owner {
        vault_token_sync(
            client,
            owner,
            &consumer,
            &token_file,
            &token_file,
            TokenSyncMode::Install,
            false,
        )
        .await?;
        report
            .push(json!({ "step": "bearer", "from": client, "host": owner, "consumer": consumer }));
    }
    grant_item_read(owner, &consumer, &role, "token", &token_file, false).await?;
    report.push(json!({ "step": "grant", "host": owner, "consumer": consumer, "role": role, "field": "token" }));

    // 2b. Every publisher this host already declares, held to the same shape
    //     before any verifier is reconciled. The repair in step 4 grants the
    //     verifier every declared publisher's role at once, so one earlier
    //     publisher whose item was minted under a random id, or named after
    //     its product without the role tag, refused every later declaration
    //     and every build enrollment, each of which then withdrew its own
    //     declaration. One listing says which items are out of shape; only
    //     those are written, and a bearer is minted only for a declared
    //     publisher whose vault holds no item at all.
    let publishers = crate::config::release_api_publishers().map_err(|problems| {
        CmdError::click(format!(
            "invalid release_api.publishers: {}",
            problems.join("; ")
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let others = publishers
        .iter()
        .filter(|(other, _)| other.as_str() != product)
        .map(|(_, publisher)| publisher.item().to_string())
        .collect::<Vec<_>>();
    for item in named_role_items_out_of_shape(owner, &others).await? {
        let payload = json!({
            "schema": "skarbiec.item.v2",
            "kind": "token",
            "fields": { "token": mint_bearer() },
            "context": { "product": item, "role": "release-publisher" },
        })
        .to_string();
        let stored = write_named_role_item(owner, &item, &item, "token", &payload).await?;
        grant_item_read(owner, &consumer, &item, "token", &token_file, false).await?;
        report.push(json!({
            "step": "declared-item", "host": owner, "role": item, "stored": stored,
        }));
    }

    // 3. The declaration, on every host that serves or submits releases. The
    //    write is guarded: a host whose vault plays no such role refuses it.
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
    //    The reconciliation reads the authoritative publisher items from the
    //    vault on the machine it runs on, so a client that reads the owner's
    //    vault through secrets.skarbiec.url runs it on the owner.
    //    A declaration no verifier accepts closes that host's release
    //    publication boundary, so any failure retracts every declaration
    //    written above before the error is returned.
    for host in &declared_on {
        // A client that reads the owner's vault holds no vault a verifier
        // could be reconciled against: the repair would judge a retired local
        // copy. Its declaration only lets its own build and release submit
        // find the publisher.
        if client_reads_owner && host == client {
            report.push(json!({ "step": "verifier", "host": host, "repair": "not needed: reads the vault on the owner", "owner": owner }));
            continue;
        }
        let arguments = [
            "repair",
            "stado",
            "--step",
            "release-verifier",
            "--target",
            host.as_str(),
            "--apply",
        ];
        let repaired = if client_reads_owner {
            crate::cli::host::remote_stado_output(owner, &arguments)
                .await
                .map(|_| ())
        } else {
            let repair = std::process::Command::new(std::env::current_exe()?)
                .args(arguments)
                .output()?;
            if repair.status.success() {
                Ok(())
            } else {
                // The repair is this binary's own `repair stado` run: it did
                // not complete on this host, the host's outage.
                Err(CmdError::unreachable(
                    String::from_utf8_lossy(&repair.stderr).trim().to_string(),
                ))
            }
        };
        if let Err(error) = repaired {
            let mut retracted = Vec::new();
            for declared_host in &declared_on {
                let outcome = crate::cli::host::remote_stado_output(
                    declared_host,
                    &["config", "unset", &key],
                )
                .await;
                retracted.push(match outcome {
                    Ok(_) => format!("{declared_host}: retracted"),
                    Err(retract) => format!("{declared_host}: NOT retracted ({retract})"),
                });
            }
            return Err(error
                .within(format!(
                    "release-verifier repair for {host}{} failed after the declaration was written",
                    if client_reads_owner {
                        format!(" (run on the vault owner {owner})")
                    } else {
                        String::new()
                    }
                ))
                .also(format!(
                    "{key} was withdrawn so no verifier fails closed on it: {}",
                    retracted.join("; ")
                )));
        }
        report.push(json!({ "step": "verifier", "host": host, "repair": "release-verifier", "ran_on": if client_reads_owner { owner } else { client } }));
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
                &json!({ "product": product, "role": role, "steps": report })
            )?
        );
    } else {
        println!(
            "{product}: publisher role {role} {} on {owner}; {consumer} may read it; declared on {}; release verifier reconciled",
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
    eprintln!("{product}: this host declares no release publisher for it; declaring it now");
    declare_publisher_on_fleet(product).await
}

/// `declare_publisher` between the fleet's vault owner and this host, with the
/// command it ran named in the refusal. Also used for a declared publisher
/// whose item Stado cannot read (`enroll::publishers`).
pub(super) async fn declare_publisher_on_fleet(product: &str) -> Result<(), CmdError> {
    let (owner, client) = fleet_hosts().await?;
    eprintln!("{product}: declaring the publisher (vault owner {owner}, release client {client})");
    declare_publisher(product, &owner, &client, &[], &[], false)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "declaring {product}'s release publisher failed \
                 (stado release catalog declare-publisher {product} --owner {owner} \
                 --client {client}): {error}"
            ))
        })
}
