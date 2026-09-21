//! One qualification pass: build the head once, and answer for every
//! delivery it carries.
//!
//! This is the only place a delivery turns into a build. It is deliberate —
//! an operator or a scheduled pass says "prove what has piled up" — and it
//! spends exactly one build per platform no matter how many deliveries are
//! waiting, which is the whole point: twelve changes cost the fleet one
//! build, not twelve.

use serde_json::{json, Value};

use crate::cli::builds::jobs::submit_recipe_build;
use crate::cli::delivery::{
    deliveries_array, fetch_mutation_document, passes_array, print_json, read_registry,
};
use crate::cli::CmdError;
use crate::models::isoformat_utc;
use crate::targets::{read_build_recipes, Delivery, DeliveryState, QualificationPass};

/// Start one pass for `product`.
///
/// The head that gets built is whatever the recipe's branch points at now,
/// not the newest delivery: a pass proves the tree as it stands, which is the
/// thing that would actually be installed. Every waiting delivery for the
/// product is bound to it, because each was pushed to that branch before the
/// head was read.
pub(super) async fn qualify(
    product: &str,
    retry_token: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    if retry_token.trim().is_empty() {
        return Err(CmdError::click("--run-id must not be empty"));
    }
    let registry = read_registry().await?;
    let recipes = read_build_recipes(&registry);
    let recipe = recipes
        .iter()
        .find(|recipe| recipe.name == product)
        .ok_or_else(|| {
            CmdError::click(format!(
                "no build recipe named {product:?}, so nothing here knows how to build and test it"
            ))
        })?;
    let head = crate::scheduler::builds::ls_remote(&recipe.repo, &recipe.branch)
        .await
        .map_err(|reason| {
            CmdError::click(format!(
                "cannot read {}@{}: {reason}",
                recipe.repo, recipe.branch
            ))
        })?;
    if head.is_empty() {
        return Err(CmdError::click(format!(
            "{}@{} has no head to build",
            recipe.repo, recipe.branch
        )));
    }

    let (mut document, generation) = fetch_mutation_document().await?;
    let waiting: Vec<Delivery> = deliveries_array(&mut document)?
        .iter()
        .filter_map(|entry| serde_json::from_value::<Delivery>(entry.clone()).ok())
        .filter(|delivery| delivery.product == product && delivery.state == DeliveryState::Waiting)
        .collect();
    if waiting.is_empty() {
        return Err(CmdError::click(format!(
            "nothing is waiting for {product}: a pass with nothing to prove would spend one of \
             the fleet's builds on nobody. Record what was pushed with `stado delivery deliver`."
        )));
    }

    let started_at = isoformat_utc(chrono::Utc::now());
    let pass_id = format!("{product}-{retry_token}");
    let (submitted, _) = submit_recipe_build(
        &mut document,
        product,
        retry_token,
        "stado delivery qualify",
    )
    .await?;
    let pass = QualificationPass {
        id: pass_id.clone(),
        product: product.to_string(),
        revision: head,
        started_at: started_at.clone(),
        jobs: submitted
            .iter()
            .map(|(platform, run)| (platform.clone(), run.job_id.clone()))
            .collect(),
        deliveries: waiting.iter().map(|delivery| delivery.id.clone()).collect(),
        status: QualificationPass::RUNNING.to_string(),
        settled_at: None,
        reason: None,
    };
    passes_array(&mut document)?.push(serde_json::to_value(&pass)?);
    bind_deliveries(&mut document, &pass)?;
    crate::cli::registry::push_document_if(&document, &generation).await?;

    if json_output {
        return print_json(&json!({
            "pass": serde_json::to_value(&pass)?,
            "deliveries": pass.deliveries,
        }));
    }
    println!(
        "{product}: qualifying {} delivery(ies) at {} in pass {pass_id}",
        pass.deliveries.len(),
        &pass.revision
    );
    for (platform, job_id) in &pass.jobs {
        println!("  {platform}: build job {job_id}");
    }
    Ok(())
}

/// Mark every delivery the pass answers for as under qualification, in the
/// same document the pass is written into.
fn bind_deliveries(document: &mut Value, pass: &QualificationPass) -> Result<(), CmdError> {
    for entry in deliveries_array(document)?.iter_mut() {
        let Ok(mut delivery) = serde_json::from_value::<Delivery>(entry.clone()) else {
            continue;
        };
        if !pass.deliveries.contains(&delivery.id) {
            continue;
        }
        delivery.state = DeliveryState::Qualifying;
        delivery.pass = Some(pass.id.clone());
        *entry = serde_json::to_value(&delivery)?;
    }
    Ok(())
}
