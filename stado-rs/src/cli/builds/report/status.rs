//! `stado builds status`: one recipe in full, with the live queue state of
//! each per-platform build job it recorded.

use std::collections::BTreeMap;

use serde_json::json;

use crate::cli::builds::report::{reported_platforms, run_header, run_row};
use crate::cli::builds::{print_json, read_registry, recipe_index, recipe_json};
use crate::cli::CmdError;
use crate::queue::runs::ALL_PREFIXES;
use crate::queue::submit::default_store;
use crate::targets::read_build_recipes;

pub(in crate::cli::builds) async fn status(name: &str, json: bool) -> Result<(), CmdError> {
    let registry = read_registry().await?;
    let recipes = read_build_recipes(&registry);
    let index = recipe_index(&recipes, name)?;
    let recipe = &recipes[index];
    let mut job_states: BTreeMap<String, Option<&'static str>> = BTreeMap::new();
    if !recipe.runs.is_empty() {
        let store = default_store(crate::config::bucket()).await?;
        for (platform, run) in &recipe.runs {
            let mut found = None;
            for state in ALL_PREFIXES {
                if store.read_job(state, &run.job_id).await?.is_some() {
                    found = Some(state);
                    break;
                }
            }
            job_states.insert(platform.clone(), found);
        }
    }
    if json {
        return print_json(&json!({
            "recipe": recipe_json(recipe)?,
            "job_states": job_states,
        }));
    }
    println!("name:         {}", recipe.name);
    println!("source:       {}@{}", recipe.repo, recipe.branch);
    println!("command:      {}", recipe.command);
    println!("artifacts:    {}", recipe.artifacts.join(", "));
    println!("platforms:    {}", recipe.platforms.join(", "));
    println!("enabled:      {}", recipe.enabled);
    println!("auto-declare: {}", recipe.auto_declare);
    println!("interval:     {}s", recipe.interval_seconds);
    println!(
        "last seen:    {}",
        recipe.last_seen_ref.as_deref().unwrap_or("-")
    );
    let platforms = reported_platforms(recipe);
    if platforms.is_empty() {
        println!("runs:         none (no platforms declared)");
        return Ok(());
    }
    println!("runs:");
    println!("{}  JOB STATE", run_header());
    for platform in &platforms {
        let run = recipe.runs.get(platform);
        let state = match job_states.get(platform) {
            Some(Some(state)) => state,
            Some(None) => "(not in queue)",
            None => "-",
        };
        println!("  {}  {state}", run_row(platform, run));
    }
    for (platform, run) in &recipe.runs {
        for uri in &run.artifact_uris {
            println!("artifact:     {platform} {uri}");
        }
    }
    Ok(())
}
