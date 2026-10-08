//! `stado inference routes show`: what the registry declares against what
//! the gateway host actually serves, and the repair that restages one onto
//! the other.

use serde_json::{json, Value};

use super::{route_host, ABSENT};
use crate::cli::CmdError;
use crate::deploy::{inference::routes, production_runner};
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
/// the host, because placement is declared, not observed. `--probe-bearer-role`
/// also asks the gateway one real request per declared alias, because a table
/// that agrees with its declaration can still route every request to a
/// provider that refuses it, and only a request shows that.
pub async fn show(
    repair: bool,
    probe_bearer_role: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = schema::parse(&document).map_err(CmdError::declaration)?;
    let Some(host) = route_host(&registry) else {
        return Err(CmdError::click(
            "registry.inference declares no gateway target, so no host serves a route table",
        )
        .stating(crate::primitives::failure::FailureCode::Config));
    };
    let runner = production_runner();
    let target = crate::cli::canonical_host(host).await?;
    let live = routes::live(&target, &runner)
        .await
        .map_err(CmdError::from)?;
    // `stage` writes the serialized registry SECTION, not a whole registry
    // document, so the host's table has `routes` at its top level and
    // `schema::parse` — which reads `document["inference"]` — would report every
    // alias as absent rather than say it could not find the section.
    let served = match live.as_ref() {
        Some(value) => Some(
            serde_json::from_value::<schema::Registry>(value.clone()).map_err(|error| {
                CmdError::click(format!("the gateway route table is invalid: {error}"))
                    .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?,
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
    let mut not_answering = Vec::new();
    if let Some(role) = probe_bearer_role {
        let declared: Vec<String> = registry.routes.keys().cloned().collect();
        let report = routes::answers(&target, &declared, role, &runner)
            .await
            .map_err(CmdError::from)?;
        let probed = report["aliases"].as_array().cloned().into_iter().flatten();
        for answer in probed {
            let Some(alias) = answer.get("alias").and_then(Value::as_str) else {
                continue;
            };
            // Brama's probe names the model that answered only for a request
            // a model served; a refusal or a transport error carries none.
            let answered = answer.get("model").is_some();
            if !answered {
                not_answering.push(alias.to_string());
            }
            if let Some(row) = rows.iter_mut().find(|row| row["alias"] == json!(alias)) {
                row["answers"] = json!(answered);
                row["probe"] = answer.clone();
            }
        }
    }
    let repaired = if repair && !diverged.is_empty() {
        let transaction = routes::transaction(&registry).map_err(CmdError::from)?;
        let staged = routes::stage(&target, &registry, &transaction, &runner)
            .await
            .map_err(CmdError::from)?;
        if !routes::ready(&staged, "routes_staged") {
            return Err(CmdError::click("could not stage inference routes")
                .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        let committed = routes::commit(&target, &transaction, &runner)
            .await
            .map_err(CmdError::from)?;
        if !routes::ready(&committed, "routes_committed") {
            return Err(CmdError::refused(
                "the gateway refused the declared route table",
            ));
        }
        Some(routes::summary(&transaction, staged, committed))
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
                "not_answering": not_answering,
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
            let answers = match row.get("answers").and_then(Value::as_bool) {
                Some(true) => " answers",
                Some(false) => " NOT ANSWERING",
                None => "",
            };
            println!(
                "{alias:<32} {verdict:<9} declared={} serving={}{answers}",
                row["declared"], row["serving"]
            );
            if row.get("answers").and_then(Value::as_bool) == Some(false) {
                println!("{:<32} {}", "", row["probe"]);
            }
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
        // The gateway does not serve what is declared: the host's state is
        // the outage, which --repair converges.
        return Err(CmdError::unreachable(format!(
            "the gateway on {} does not serve the declared route table for {}; \
             re-run with --repair to stage and commit the declaration",
            target.name,
            diverged.join(",")
        )));
    }
    if !not_answering.is_empty() {
        // The table is served as declared, and the destination behind these
        // aliases refuses: the route has to point somewhere that answers.
        return Err(CmdError::unreachable(format!(
            "the gateway on {} routes {} to destinations that did not answer the probe; each line \
             above carries the refusal, and `stado inference route set` points an alias at one \
             that answers",
            target.name,
            not_answering.join(",")
        )));
    }
    Ok(())
}
