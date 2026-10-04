use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::host::secrets::vault::item::read_vault_phase;

/// Refuse a publisher declaration whose Skarbiec item the host does not hold.
///
/// `release_api.publishers.<product>` names an item the host's release verifier
/// must be able to read. The host computes its grant's item set from that
/// declaration and compares the two as sets, so one declaration whose item was
/// never minted takes the WHOLE release publication boundary down: every
/// `/api/object` read of a `system/release-catalog/*` key then answers 401 or
/// 503, for every product, including the ones publishing perfectly.
///
/// A product declared as a publisher without its product-named item
/// in the vault makes the boundary report
/// `release verifier grant item set mismatch (missing=[...], unexpected=[...])`
/// on the host and nowhere an operator is looking. A publisher declaration
/// whose item does not exist is the defect, never the missing item: mint the
/// item first, then declare it.
///
/// The item is looked for in the vault the host's verifier reads: the fleet
/// vault on its owner, the registry's `skarbiec` active host. Reading the
/// declaring host's own file would refuse an item `enroll` has just minted
/// on the owner, because a non-owner's local copy no longer receives the
/// owner's items.
pub(super) async fn refuse_unminted_publisher(
    target: &str,
    key: &str,
    value: &str,
) -> Result<(), CmdError> {
    let Some(product) = key.strip_prefix("release_api.publishers.") else {
        return Ok(());
    };
    if product.is_empty() || product.contains('.') {
        return Ok(());
    }
    let Some(item) = serde_json::from_str::<Value>(value)
        .ok()
        .and_then(|declared| {
            declared
                .get("item")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
    else {
        return Ok(());
    };
    let vault_host = crate::cli::directory::active_host("skarbiec")
        .await?
        .unwrap_or_else(|| target.to_string());
    let resolved = crate::cli::canonical_host(&vault_host).await?;
    let runner = crate::deploy::production_runner();
    let environment = crate::deploy::host_channel::run_command(
        &resolved,
        "printf '%s\\n' \"${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}\"",
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !environment.ok() {
        return Err(CmdError::click(format!(
            "{}: the vault path could not be read, so it cannot be said whether {item} exists: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&environment, "remote command failed")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let vault = environment.stdout.trim().to_string();
    if vault.is_empty() {
        return Err(
            CmdError::click(format!("{}: the vault path is empty", resolved.name))
                .stating(crate::primitives::failure::FailureCode::Config),
        );
    }
    let record = read_vault_phase(&resolved, &vault, &item, &runner)
        .await
        .map_err(CmdError::click)?;
    if record.state != "absent" {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "the fleet vault on {owner} does not hold Skarbiec item {item:?}, so declaring publisher \
         {product:?} on {target} would close that host's whole release publication boundary: its \
         release verifier compares the declared publisher set against its grant's item set, and \
         one unmintable name makes them unequal for every product, answering 401 or 503 to every \
         release-catalog read on the fleet. Mint the item on {owner} first - `stado credentials \
         item put --host {owner} {item} --type token` - then declare it and run `stado repair \
         stado --step release-verifier --target {target} --apply`.",
        owner = resolved.name
    )))
}

/// Say, at declaration time, what an object namespace without a grant costs.
///
/// The object verifier needs a read grant for every declared namespace item.
/// An uncovered item closes the whole object authorization boundary.
/// Warn while the declaration is written, naming the uncovered item and
/// both configuration owners: object-verifier repair derives its required
/// set from the caller's configuration, not only the target's configuration.
pub(super) fn warn_unbacked_object_namespace(target: &str, key: &str, value: &str) {
    let Some(namespace) = key.strip_prefix("object_api.namespaces.") else {
        return;
    };
    if namespace.is_empty() || namespace.contains('.') {
        return;
    }
    let item = serde_json::from_str::<Value>(value)
        .ok()
        .and_then(|declared| {
            declared
                .get("item")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let Some(item) = item else {
        return;
    };
    let covered = crate::config::object_api_namespaces()
        .map(|namespaces| {
            crate::config::object_verifier_items(namespaces)
                .iter()
                .any(|held| held == &item)
        })
        .unwrap_or(false);
    if covered {
        eprintln!(
            "note: {target}'s object verifier grant must cover {item:?} for namespace \
             {namespace:?}; this machine declares it too, so reconcile the host with: stado \
             repair stado --step object-verifier --target {target} --apply"
        );
        return;
    }
    eprintln!(
        "warning: namespace {namespace:?} on {target} names Skarbiec item {item:?}, and this \
         machine's own object_api.namespaces does not declare it. Until the host's object verifier \
         grant covers that item its WHOLE object authorization boundary closes — every \
         /api/object read answers 503, not just this namespace — and the failure surfaces at the \
         next restart of anything that reads the registry, including the release agent that \
         publishes every stable bind. The declared object-verifier repair computes the item set \
         from THIS machine's configuration, so declare the namespace here as well and then run: \
         stado config set {key} '<the same JSON>' && stado repair stado --step object-verifier \
         --target {target} --apply"
    );
}

/// The same warning for the other three verifier maps.
///
/// `object_api.namespaces` was the only map that said anything at declaration
/// time, and the other three fail exactly the same way: a publisher, client or
/// deployer whose Skarbiec item is outside the host's verifier grant closes
/// that verifier, and `stado doctor` then answers `release verifier grant item
/// set mismatch (missing=[...])` — hours after the declaration, blocking a
/// release train, and repaired each time by the one command this note names.
///
/// Only the remedy differs per map, so only the remedy is looked up here.
pub(super) fn warn_unbacked_verifier_item(target: &str, key: &str, value: &str) {
    let maps: [(&str, &str); 3] = [
        (
            "release_api.publishers.",
            "stado repair stado --step release-verifier --target {target} --apply",
        ),
        (
            "machine_api.clients.",
            "stado repair stado --step object-verifier --target {target} --apply",
        ),
        (
            "service_api.deployers.",
            "stado repair stado --step service-verifier --target {target} --apply",
        ),
    ];
    let Some((name, remedy)) = maps.iter().find_map(|(prefix, remedy)| {
        key.strip_prefix(prefix)
            .filter(|name| !name.is_empty() && !name.contains('.'))
            .map(|name| (name, *remedy))
    }) else {
        return;
    };
    let Some(item) = serde_json::from_str::<Value>(value)
        .ok()
        .and_then(|declared| {
            declared
                .get("item")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
    else {
        return;
    };
    let remedy = remedy.replace("{target}", target).replace("{name}", name);
    eprintln!(
        "note: {target}'s verifier grant must cover Skarbiec item {item:?} for {key}. Until it \
         does, that whole verifier fails closed — `stado doctor` reports a grant item set \
         mismatch and every gateway read it authorizes answers 403 — so run: {remedy}"
    );
}
