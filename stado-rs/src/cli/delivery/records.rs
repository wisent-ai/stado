//! Recording a delivery, and the refusals that keep the register honest.

use serde_json::Value;

use crate::cli::delivery::{deliveries_array, fetch_mutation_document, print_json, read_registry};
use crate::cli::CmdError;
use crate::models::isoformat_utc;
use crate::targets::{read_build_recipes, Delivery, DeliveryState};

/// How much of a commit a delivery identifier carries.
const ID_REVISION: usize = 8;

/// Record one pushed revision as waiting for proof.
///
/// The product must be a declared build recipe, because the recipe is what
/// knows how to build and test it: a delivery for a product nothing can
/// qualify would wait forever, and waiting forever is indistinguishable from
/// being forgotten, which is the thing this register exists to end.
pub(super) async fn deliver(
    product: &str,
    revision: &str,
    summary: Option<String>,
    task: Option<String>,
    session: Option<String>,
    json: bool,
) -> Result<(), CmdError> {
    let revision = revision.trim().to_lowercase();
    if revision.len() < FULL_REVISION || !revision.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CmdError::click(format!(
            "--revision must be the exact full commit that was pushed, and {revision:?} is not one"
        )));
    }
    let registry = read_registry().await?;
    let recipes = read_build_recipes(&registry);
    let Some(recipe) = recipes.iter().find(|recipe| recipe.name == product) else {
        let known = recipes
            .iter()
            .map(|recipe| recipe.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(CmdError::click(format!(
            "no build recipe named {product:?}, so nothing here can qualify that revision; \
             the fleet declares: {known}"
        )));
    };
    let repo = recipe.repo.clone();

    let (mut document, generation) = fetch_mutation_document().await?;
    let entries = deliveries_array(&mut document)?;
    if let Some(existing) = entries.iter().find(|entry| {
        entry.get("product").and_then(Value::as_str) == Some(product)
            && entry.get("revision").and_then(Value::as_str) == Some(revision.as_str())
    }) {
        let state = existing
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("waiting");
        return Err(CmdError::click(format!(
            "{product} {} is already delivered and {state}",
            &revision[..ID_REVISION]
        )));
    }
    let delivered_at = isoformat_utc(chrono::Utc::now());
    let mut delivery = Delivery::new(
        format!("{product}-{}", &revision[..ID_REVISION]),
        product,
        repo,
        &revision,
        &delivered_at,
    );
    delivery.summary = summary.filter(|value| !value.trim().is_empty());
    delivery.task = task.filter(|value| !value.trim().is_empty());
    delivery.session = session.filter(|value| !value.trim().is_empty());
    let recorded = serde_json::to_value(&delivery)?;
    entries.push(recorded.clone());
    crate::cli::registry::push_document_if(&document, &generation).await?;
    if json {
        return print_json(&recorded);
    }
    println!(
        "{}: delivered {} — waiting for a qualification pass",
        delivery.product,
        delivery.short_revision()
    );
    Ok(())
}

/// A full commit is forty hexadecimal characters; anything shorter names a
/// revision the fleet would have to guess at.
const FULL_REVISION: usize = 40;

/// Mark a failed delivery as handed back to its task, so one failure reopens
/// one task once and a second reader does not file it again.
pub(super) async fn reported(id: &str, json: bool) -> Result<(), CmdError> {
    let (mut document, generation) = fetch_mutation_document().await?;
    let entries = deliveries_array(&mut document)?;
    let entry = entries
        .iter_mut()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
        .ok_or_else(|| CmdError::click(format!("no delivery {id:?} in the register")))?;
    let mut delivery: Delivery = serde_json::from_value(entry.clone())
        .map_err(|error| CmdError::click(format!("delivery {id:?} does not parse: {error}")))?;
    if delivery.state != DeliveryState::Failed {
        return Err(CmdError::click(format!(
            "delivery {id:?} is {} — only a failed delivery is handed back to its task",
            delivery.state.as_str()
        )));
    }
    delivery.reported = true;
    *entry = serde_json::to_value(&delivery)?;
    crate::cli::registry::push_document_if(&document, &generation).await?;
    if json {
        return print_json(&serde_json::to_value(&delivery)?);
    }
    println!("{id}: recorded as handed back to its task");
    Ok(())
}
