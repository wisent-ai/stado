//! `stado builds add`: every field checked, then one fenced write that
//! appends a disabled recipe the poller will not build until it is enabled.

use std::collections::BTreeMap;

use crate::cli::builds::declaration::checks::{
    canonical_platforms, check_artifacts, check_branch, check_command, check_interval_seconds,
    check_repo, is_recipe_name,
};
use crate::cli::builds::declaration::AUTO_DECLARE_ON;
use crate::cli::builds::{
    builds_array, entry_name, fetch_mutation_document, print_json, recipe_json,
};
use crate::cli::CmdError;
use crate::targets::BuildRecipe;

#[allow(clippy::too_many_arguments)]
pub(in crate::cli::builds) async fn add(
    name: &str,
    repo: &str,
    branch: &str,
    command: &str,
    artifacts: Vec<String>,
    platforms: Vec<String>,
    auto_declare: bool,
    interval_seconds: u64,
    json: bool,
) -> Result<(), CmdError> {
    if !is_recipe_name(name) {
        return Err(CmdError::usage(
            "--name must be kebab-case: lowercase letters, digits and '-'",
        ));
    }
    check_repo(repo)?;
    check_branch(branch)?;
    check_command(command)?;
    check_artifacts(&artifacts)?;
    let platforms = canonical_platforms(&platforms)?;
    check_interval_seconds(interval_seconds)?;
    let (mut document, generation) = fetch_mutation_document().await?;
    let entries = builds_array(&mut document)?;
    if entries.iter().any(|entry| entry_name(entry) == Some(name)) {
        return Err(CmdError::click(format!(
            "build recipe {name:?} already exists"
        )));
    }
    let created = recipe_json(&BuildRecipe {
        name: name.to_string(),
        repo: repo.to_string(),
        branch: branch.to_string(),
        command: command.to_string(),
        artifacts,
        platforms: platforms.clone(),
        auto_declare,
        enabled: false,
        interval_seconds,
        last_seen_ref: None,
        runs: BTreeMap::new(),
    })?;
    entries.push(created.clone());
    crate::cli::registry::push_document_if(&document, &generation).await?;
    if json {
        return print_json(&created);
    }
    println!(
        "{name}: added for {} (disabled; enable with `stado builds enable {name}`)",
        platforms.join(", ")
    );
    if auto_declare {
        println!("{name}: {AUTO_DECLARE_ON}");
    }
    Ok(())
}
