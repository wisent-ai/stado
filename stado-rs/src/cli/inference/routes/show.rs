//! `stado inference routes show`: what the registry declares against what
//! the gateway host actually serves, and the repair that restages one onto
//! the other.

use serde_json::{json, Value};

use super::{click, route_host, ABSENT};
use crate::cli::CmdError;
use crate::deploy::{host_channel, inference_routes, production_runner};
use crate::inference::schema;

/// One alias, as the registry declares it and as the gateway host serves it.
fn entry(registry: &schema::Registry, alias: &str) -> Value {
    json!({
        "destination": registry.routes.get(alias),
        "fallbacks": registry.fallbacks.get(alias).cloned().unwrap_or_default(),
    })
}

/// Compare the declared route table with the one the gateway process reads,
/// and optionally restage the declaration onto the host.
///
/// [`set`] and [`remove`] write both sides in one transaction, so they cannot
/// disagree. Every other writer of the canonical registry moves the
/// declaration alone: the gateway keeps serving the table it was last staged,
/// and no command said which of the two an operator was looking at. This is
/// that command. `--repair` sends the declaration to the host through the same
/// stage-and-commit the mutations use; the registry is never rewritten from
/// the host, because placement is declared, not observed.
pub async fn show(repair: bool, json_output: bool) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = schema::parse(&document).map_err(click)?;
    let Some(host) = route_host(&registry) else {
        return Err(CmdError::click(
            "registry.inference declares no gateway target, so no host serves a route table",
        ));
    };
    let runner = production_runner();
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let live = inference_routes::live(&target, &runner)
        .await
        .map_err(click)?;
    // `stage` writes the serialized registry SECTION, not a whole registry
    // document, so the host's table has `routes` at its top level and
    // `schema::parse` — which reads `document["inference"]` — would report every
    // alias as absent rather than say it could not find the section.
    let served = match live.as_ref() {
        Some(value) => Some(
            serde_json::from_value::<schema::Registry>(value.clone())
                .map_err(|error| click(format!("the gateway route table is invalid: {error}")))?,
        ),
        None => None,
    };
    let mut aliases = registry
        .routes
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    if let Some(served) = &served {
        aliases.extend(served.routes.keys().cloned());
    }
    let mut rows = Vec::new();
    let mut diverged = Vec::new();
    for alias in &aliases {
        let declared = entry(&registry, alias);
        let serving = served
            .as_ref()
            .map(|served| entry(served, alias))
            .unwrap_or(Value::Null);
        let agrees = serving == declared;
        if !agrees {
            diverged.push(alias.clone());
        }
        rows.push(json!({
            "alias": alias,
            "declared": declared,
            "serving": serving,
            "agrees": agrees,
        }));
    }
    let repaired = if repair && !diverged.is_empty() {
        let transaction = inference_routes::transaction(&registry).map_err(click)?;
        let staged = inference_routes::stage(&target, &registry, &transaction, &runner)
            .await
            .map_err(click)?;
        if !inference_routes::ready(&staged, "routes_staged") {
            return Err(CmdError::click("could not stage inference routes"));
        }
        let committed = inference_routes::commit(&target, &transaction, &runner)
            .await
            .map_err(click)?;
        if !inference_routes::ready(&committed, "routes_committed") {
            return Err(CmdError::click(
                "the gateway refused the declared route table",
            ));
        }
        Some(inference_routes::summary(&transaction, staged, committed))
    } else {
        None
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "gateway": target.name,
                "serving_table": if live.is_some() { "present" } else { ABSENT },
                "aliases": rows,
                "diverged": diverged,
                "repair": repaired,
            }))?
        );
    } else {
        println!(
            "gateway {}: route table {}",
            target.name,
            if live.is_some() { "present" } else { ABSENT }
        );
        for row in &rows {
            let alias = row["alias"].as_str().unwrap_or_default();
            let verdict = if row["agrees"] == json!(true) {
                "agrees"
            } else {
                "DIVERGED"
            };
            println!(
                "{alias:<32} {verdict:<9} declared={} serving={}",
                row["declared"], row["serving"]
            );
        }
        if repaired.is_some() {
            println!(
                "restaged the declared table on {}: {} alias(es) repaired",
                target.name,
                diverged.len()
            );
        }
    }
    if !diverged.is_empty() && repaired.is_none() {
        return Err(CmdError::click(format!(
            "the gateway on {} does not serve the declared route table for {}; \
             re-run with --repair to stage and commit the declaration",
            target.name,
            diverged.join(",")
        )));
    }
    Ok(())
}
