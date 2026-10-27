//! `stado ci status <PRODUCT> [--revision SHA] [--json]`: what the forge's
//! continuous integration last did for a catalogued product, read from the
//! forge's own API with the identity Stado's GitHub declaration names in
//! Skarbiec.
//!
//! The product's repository is its catalog record's `owner_repository`; its
//! workflows are the ones that repository declares under `.github/workflows`
//! as the forge lists them. For each workflow the newest run — of one
//! revision when `--revision` names it — is read with its conclusion, and a
//! run that did not succeed carries every job that failed with the step
//! that failed in it, so the answer says what broke, not only that something
//! did. A product whose repository declares no workflow is answered as such,
//! which is a finding about the product, not a refusal. Skarbiec, the
//! declaration and the forge each refuse by name when they do not answer;
//! nothing is guessed in their place. The text form prints each field as
//! the forge gave it, `null` where the forge gave none.
//!
//! The answer also carries the delivery routine ([`delivery`]): its declared
//! plan, the step it is installing and for how long, the step that refused
//! with every refusal and its defects, what each step last delivered, and
//! what the main check ([`main`]) found on each step's `main`.

pub mod delivery;
mod forge;
pub mod main;

use serde_json::{json, Value};

use crate::cli::CmdError;

use forge::{answer, client, listing, newest_run, API};

/// The catalogued product's repository, `owner/name`.
fn repository(product: &str) -> Result<String, CmdError> {
    let catalog = stado_product::catalog::embedded().map_err(|error| {
        CmdError::click(format!(
            "the compiled product catalog is invalid: {error:#}"
        ))
    })?;
    let record = stado_product::catalog::product(&catalog, product).map_err(|error| {
        CmdError::usage(format!(
            "{error:#}; `stado product catalog` lists the products"
        ))
    })?;
    stado_product::catalog::text(record, "owner_repository")
        .map(str::to_string)
        .map_err(|error| CmdError::click(format!("{product}: {error:#}")))
}

/// `stado ci status`: the answer document.
pub async fn status_document(product: &str, revision: Option<&str>) -> Result<Value, CmdError> {
    let repository = repository(product)?;
    let token = crate::github_identity::credential()
        .await
        .map_err(|error| {
            CmdError::from(error).within("the forge identity Stado acts as is not readable")
        })?;
    let request = client(&token);
    let listed = answer(
        request(format!("{API}/repos/{repository}/actions/workflows")),
        &format!("the workflows of {repository}"),
    )
    .await?;
    let workflows = listing(&listed, "workflows", &repository)?;
    let mut rows = Vec::with_capacity(workflows.len());
    for workflow in workflows {
        rows.push(newest_run(&request, &repository, workflow, revision).await?);
    }
    let finding = workflows
        .is_empty()
        .then(|| format!("{repository} declares no workflow under .github/workflows, so nothing runs for it on the forge"));
    Ok(json!({
        "product": product,
        "repository": repository,
        "revision": revision,
        "workflows": rows,
        "finding": finding,
        "delivery": delivery::status()?,
    }))
}

/// `stado ci status <PRODUCT> [--revision SHA] [--json]`.
pub async fn status(product: &str, revision: Option<&str>, json: bool) -> Result<(), CmdError> {
    let document = status_document(product, revision).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&document)
                .map_err(|error| CmdError::click(error.to_string()))?
        );
        return Ok(());
    }
    println!("{product} ({})", crate::cli::reporting::field(&document["repository"]));
    if let Some(finding) = document["finding"].as_str() {
        println!("  {finding}");
    }
    delivery::print_status(&document["delivery"]);
    for row in document["workflows"].as_array().into_iter().flatten() {
        let workflow = crate::cli::reporting::field(&row["workflow"]);
        match row["run"].as_object() {
            None => println!("  {workflow}: {}", crate::cli::reporting::field(&row["finding"])),
            Some(run) => {
                use crate::cli::reporting::field;
                println!(
                    "  {workflow}: concluded {} (status {}) run {} on {} at {} {}",
                    field(&run["conclusion"]),
                    field(&run["status"]),
                    field(&run["number"]),
                    field(&run["revision"]),
                    field(&run["updatedAt"]),
                    field(&run["url"]),
                );
                for job in run["failedJobs"].as_array().into_iter().flatten() {
                    println!(
                        "    failed job {}: step {} ({})",
                        field(&job["name"]),
                        field(&job["failedStep"]["name"]),
                        field(&job["url"])
                    );
                }
            }
        }
    }
    Ok(())
}
