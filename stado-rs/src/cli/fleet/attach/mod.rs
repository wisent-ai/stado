//! Attaching a registered machine to the store this host reads: what
//! bootstrap does on a remote target once its Stado binary is in place and
//! before its host unit is installed.
//!
//! A machine can arrive carrying its own standalone Stado, whose queue and
//! registry live in a device-local store that names the machine by its
//! hostname. Registered under another name and given the fleet's unit, its
//! `serve --auto` resolved that store's name and refused the registered one,
//! and its worker and health beacon never reached the fleet. Attaching
//! points the machine at the store this host declares (`storage.*`), in four
//! steps, each idempotent: its resolver key is authorized on the directory
//! authority; its resolver is declared the adapter this host reaches the
//! fleet object API through, on ports the machine hands out; the bearer of
//! the fleet namespace is written to its token file; and its `storage`
//! configuration is rewritten to the fleet store. Its device-local store is
//! not touched: it stays at the path its configuration names, as the
//! machine's own object API root, and that path is reported.

mod config;
mod session;

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::primitives::failure::FailureCode;
use crate::targets::ComputeTarget;

/// Where an attached machine keeps the fleet object API's bearer.
const TOKEN_FILE: &str = "$HOME/.stado/queue-object-api-token";
/// The bearer field of a namespace's object API item.
const BEARER_FIELD: &str = "token";

/// The fleet store as this host declares it.
struct FleetStore {
    url: String,
    service: String,
    consumer: String,
    namespace: String,
}

/// This host's store, refused unless it is a Stado object API reached
/// through an adapter: that is the only store a second machine can share.
fn fleet_store() -> Result<FleetStore, CmdError> {
    let refused = |why: String| {
        CmdError::click(format!(
            "this host cannot attach a machine to its store: {why}; bootstrap from a host whose \
             storage.backend is stado and whose storage.stado.url is \
             stado://service/<service>?consumer=<consumer>"
        ))
        .stating(FailureCode::Config)
    };
    let backend = crate::config::wc_storage_backend();
    if backend != crate::capabilities::StorageAdapter::StadoObject.id() {
        return Err(refused(format!("storage.backend is {backend:?}")));
    }
    let url = crate::config::wc_stado_storage_url_configured();
    let parsed = url::Url::parse(url)
        .map_err(|error| refused(format!("storage.stado.url {url:?} is not a URL ({error})")))?;
    let service = parsed.path().trim_start_matches('/').to_string();
    let consumer = parsed
        .query_pairs()
        .find(|(key, _)| key == "consumer")
        .map(|(_, value)| value.into_owned());
    let (true, Some("service"), Some(consumer), false) = (
        parsed.scheme() == "stado",
        parsed.host_str(),
        consumer,
        service.is_empty(),
    ) else {
        return Err(refused(format!(
            "storage.stado.url is {url:?}, not an adapter address"
        )));
    };
    let namespace = crate::config::wc_stado_storage_namespace().to_string();
    if namespace.is_empty() {
        return Err(refused("storage.stado.namespace is empty".to_string()));
    }
    Ok(FleetStore {
        url: url.to_string(),
        service,
        consumer,
        namespace,
    })
}

/// Attach `target` to this host's store, saying each step through `echo`.
pub(crate) async fn attach(
    target: &ComputeTarget,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), CmdError> {
    let store = fleet_store()?;
    let here = crate::cli::release_catalog::this_host().await?;
    if here == target.name {
        return Ok(());
    }
    let document = crate::cli::registry::fetch_document().await?;
    let registry = crate::targets::load_registry_from_str(&serde_json::to_string(&document)?)
        .map_err(|error| CmdError::click(error.to_string()).stating(FailureCode::Config))?;
    let key = crate::deploy::host_access::resolver_key::authorize(&registry, &target.name)
        .await
        .map_err(CmdError::from)?;
    echo(&format!(
        "[attach] {}: resolver key {} on the directory authority ({})",
        target.name,
        key["key_state"].as_str().unwrap_or("reported"),
        key["authorized_keys"].as_str().unwrap_or("reported")
    ));
    declare_resolver(target, &here, &store, &document, runner, echo).await?;
    let item = crate::config::object_api_namespace(&store.namespace)
        .map(|policy| policy.item().to_string())
        .ok_or_else(|| {
            CmdError::click(format!(
                "this host declares no object_api.namespaces.{} item, so the bearer of the \
                 fleet namespace cannot be found",
                store.namespace
            ))
            .stating(FailureCode::Config)
        })?;
    // Read as named: the namespace declaration names the item, and Stado's
    // own reader reads it as the object API's verifier does, where a read by
    // role finds nothing unless the item also carries a role tag.
    let secret = crate::skarbiec::Client::stado()?
        .read_declared_string(&item, BEARER_FIELD)
        .await?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CmdError::missing(format!(
                "{item} holds no {BEARER_FIELD} field, so the fleet namespace has no bearer to give"
            ))
        })?;
    let written =
        crate::deploy::service::write_token_file_on_host(target, TOKEN_FILE, &secret, runner)
            .await
            .map_err(CmdError::from)?;
    drop(secret);
    if !written.succeeded("token_file_synced") {
        return Err(CmdError::click(format!(
            "{}: the fleet object API bearer could not be written to {TOKEN_FILE}: {}",
            target.name,
            written.failure()
        ))
        .stating(FailureCode::InfraDown));
    }
    echo(&format!(
        "[attach] {}: bearer of namespace {} written to {TOKEN_FILE}",
        target.name, store.namespace
    ));
    // The local store is read before anything is written: `config init`
    // writes its template, whose local path is the default one.
    let local = config::local_store(target, runner).await?;
    config::ensure_file(target, runner, echo).await?;
    session::declare_keepalive(target, echo).await?;
    let storage = json!({
        "backend": crate::capabilities::StorageAdapter::StadoObject.id(),
        "local": { "path": local },
        "stado": {
            "url": store.url,
            "namespace": store.namespace,
            "token_file": TOKEN_FILE.replacen("$HOME", "~", 1),
        },
    });
    crate::cli::host::write_host_config(&target.name, "storage", &storage.to_string()).await?;
    echo(&format!(
        "[attach] {}: storage is the fleet store {} (namespace {}); its own store stays at {local}",
        target.name, store.url, store.namespace
    ));
    Ok(())
}

/// The adapter `resolver` declares for `service` and `consumer`.
fn adapter_in<'a>(resolver: &'a Value, store: &FleetStore) -> Option<&'a Value> {
    resolver["adapters"].as_array()?.iter().find(|adapter| {
        adapter["service"] == store.service.as_str()
            && adapter["consumer"] == store.consumer.as_str()
    })
}

fn resolver_of<'a>(document: &'a Value, name: &str) -> &'a Value {
    document["targets"]
        .as_array()
        .and_then(|targets| targets.iter().find(|entry| entry["name"] == name))
        .map_or(&Value::Null, |entry| &entry["service_resolver"])
}

/// Declare on `target` the resolver adapter this host reaches the fleet
/// store through: this host's own declaration of that adapter (and of the
/// resolver's refresh and max-stale) with addresses the machine hands out.
async fn declare_resolver(
    target: &ComputeTarget,
    here: &str,
    store: &FleetStore,
    document: &Value,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), CmdError> {
    if adapter_in(resolver_of(document, &target.name), store).is_some() {
        return Ok(());
    }
    let ours = resolver_of(document, here);
    let template = adapter_in(ours, store).cloned().ok_or_else(|| {
        CmdError::click(format!(
            "{here} declares no resolver adapter for {} as {}, so there is no declaration to \
             give {}",
            store.service, store.consumer, target.name
        ))
        .stating(FailureCode::Config)
    })?;
    let adapter_port = crate::cli::directory::host_free_port(target, runner).await?;
    let api_port = crate::cli::directory::host_free_port(target, runner).await?;
    if adapter_port == api_port {
        return Err(CmdError::click(format!(
            "{}: the host handed out port {adapter_port} twice; run the bootstrap again",
            target.name
        ))
        .stating(FailureCode::InfraDown));
    }
    let loopback = std::net::Ipv4Addr::LOCALHOST;
    crate::cli::registry::commit_document(|current| {
        let mut next = current.clone();
        let entry = next["targets"]
            .as_array_mut()
            .and_then(|targets| {
                targets
                    .iter_mut()
                    .find(|entry| entry["name"] == target.name.as_str())
            })
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::missing(format!("target '{}' not found in registry", target.name))
            })?;
        let resolver = entry.entry("service_resolver").or_insert_with(|| json!({}));
        if resolver.is_null() {
            *resolver = json!({});
        }
        let resolver = resolver.as_object_mut().ok_or_else(|| {
            CmdError::declaration(format!(
                "{}.service_resolver must be an object",
                target.name
            ))
        })?;
        for copied in ["refresh_seconds", "max_stale_seconds"] {
            if let (false, Some(value)) = (resolver.contains_key(copied), ours.get(copied)) {
                resolver.insert(copied.to_string(), value.clone());
            }
        }
        resolver
            .entry("api_bind")
            .or_insert_with(|| json!(format!("{loopback}:{api_port}")));
        let mut adapter = template.clone();
        adapter["bind"] = json!(format!("{loopback}:{adapter_port}"));
        let adapters = resolver.entry("adapters").or_insert_with(|| json!([]));
        adapters
            .as_array_mut()
            .ok_or_else(|| {
                CmdError::declaration(format!(
                    "{}.service_resolver.adapters must be a list",
                    target.name
                ))
            })?
            .push(adapter.clone());
        crate::service_resolution::advance_generation(&mut next).map_err(CmdError::declaration)?;
        Ok(next)
    })
    .await?;
    echo(&format!(
        "[attach] {}: resolver adapter {} for {} declared on {loopback}:{adapter_port}, as {here} \
         declares it",
        target.name, store.service, store.consumer
    ));
    Ok(())
}
