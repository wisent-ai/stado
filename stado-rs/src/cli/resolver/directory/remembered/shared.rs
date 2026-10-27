//! Whether a host's registry store is a service of the registry authority's
//! host, read from this host's last-known-good copy.
//!
//! On a host whose store is an object API reached through this host's
//! resolver adapter, a registry read that cannot have the authority's
//! snapshot goes on to that store. When the object API is placed on the
//! authority's host, and a read of that host less than one refresh interval
//! ago got no snapshot, the store read stands on the same silent host: it
//! waited the relay's own minutes before the reader reached its
//! last-known-good copy. And a host that is the authority by its own copy
//! while its store is placed elsewhere reads a store the authority does not
//! hold.

use serde_json::Value;

use crate::cli::resolver::directory::document::kept_last_good_document;
use crate::cli::resolver::directory::source::current_target;
use crate::service_resolution::ServiceDirectory;

/// What this host's copy says about the store at `store_url`.
struct StoreRoute {
    /// This host's target name.
    here: String,
    /// The service the store's adapter carries, and where it is placed.
    service: String,
    placed_on: String,
    directory: ServiceDirectory,
}

/// The copy, its directory, this host and the service the adapter bound at
/// `store_url`'s host and port carries; `None` when this host keeps no copy,
/// the copy declares no directory, or no adapter of this host is that bind.
fn store_route(store_url: &str) -> Result<Option<StoreRoute>, String> {
    let Some(copy): Option<Value> = kept_last_good_document()? else {
        return Ok(None);
    };
    let Some(directory) = crate::service_resolution::directory(&copy)? else {
        return Ok(None);
    };
    let here = current_target(&copy)?;
    let config = crate::service_resolution::resolver_config(&copy, &here)?;
    let url = url::Url::parse(store_url).map_err(|error| error.to_string())?;
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return Ok(None);
    };
    let bind = format!("{host}:{port}");
    let Some(adapter) = config.adapters.iter().find(|adapter| adapter.bind == bind) else {
        return Ok(None);
    };
    let Some(route) = directory.services.get(&adapter.service) else {
        return Ok(None);
    };
    Ok(Some(StoreRoute {
        here,
        service: adapter.service.clone(),
        placed_on: route.active_host.clone(),
        directory,
    }))
}

/// The sentence saying the store at `store_url` is served by the registry
/// authority's host, which a read less than one interval ago found silent;
/// `None` when this host keeps no copy naming an authority, nothing stands
/// for it, or the store is not a service placed on it. A copy or a resolver
/// declaration that cannot be read is said on stderr, and the store is read.
pub(crate) fn store_on_silent_authority(store_url: &str) -> Option<String> {
    let judged = store_route(store_url).map(|route| {
        let route = route?;
        let authority = &route.directory.authority.target;
        let standing = super::unanswered::standing(authority)?;
        (route.placed_on == *authority).then(|| {
            format!(
                "the registry store {store_url} is {} on {authority}, the registry authority's \
                 host, and {standing}",
                route.service
            )
        })
    });
    match judged {
        Ok(sentence) => sentence,
        Err(cause) => {
            eprintln!(
                "stado: whether the registry store {store_url} is served by the registry \
                 authority's host cannot be told, so it is read: {cause}"
            );
            None
        }
    }
}

/// The sentence saying this host is the registry authority by its own copy
/// while the registry store it reads is a service the directory places on
/// another host: readers here then read a store the authority does not hold,
/// and after an authority move that store can still answer the old
/// authority's document. `None` when this host is not the authority, the
/// store is not one of its adapters, or the service is placed here. A copy
/// or declaration that cannot be read is the error.
pub(crate) fn authority_store_elsewhere(store_url: &str) -> Result<Option<String>, String> {
    let Some(route) = store_route(store_url)? else {
        return Ok(None);
    };
    let here = &route.here;
    if route.directory.authority.target != *here || route.placed_on == *here {
        return Ok(None);
    }
    Ok(Some(format!(
        "this host, {here}, is the registry authority by its last-known-good copy, but the \
         registry store it reads ({store_url}) is {} on {}; readers here read a store the \
         authority does not hold — bring them together: stado placement move --authority \
         --to-host {} run on {}, or stado placement role move {} --to-host {here} --reason <why>",
        route.service, route.placed_on, route.placed_on, route.placed_on, route.service
    )))
}
