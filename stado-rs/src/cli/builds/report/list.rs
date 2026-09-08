//! `stado builds list`: every recipe in the registry, each with its
//! per-platform runs underneath it.

use crate::cli::builds::report::{reported_platforms, run_header, run_row};
use crate::cli::builds::{print_json, read_registry};
use crate::cli::CmdError;
use crate::targets::read_build_recipes;

pub(in crate::cli::builds) async fn list(json: bool) -> Result<(), CmdError> {
    let registry = read_registry().await?;
    let recipes = read_build_recipes(&registry);
    if json {
        return print_json(&serde_json::to_value(&recipes)?);
    }
    if recipes.is_empty() {
        println!("(no build recipes; add one with `stado builds add`)");
        return Ok(());
    }
    println!(
        "{:<24} {:<44} {:<8} {:<13} LAST SEEN",
        "NAME", "REPO@REF", "ENABLED", "AUTO-DECLARE"
    );
    println!("{}", "-".repeat(110));
    for recipe in &recipes {
        let source = format!("{}@{}", recipe.repo, recipe.branch);
        let seen = recipe
            .last_seen_ref
            .as_deref()
            .map(|sha| sha.chars().take(8).collect::<String>())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<24} {:<44} {:<8} {:<13} {seen}",
            recipe.name, source, recipe.enabled, recipe.auto_declare
        );
        let platforms = reported_platforms(recipe);
        if platforms.is_empty() {
            println!("  (no platforms declared; re-add the recipe with --platform)");
            continue;
        }
        println!("{}", run_header().trim_end());
        for platform in &platforms {
            println!(
                "  {}",
                run_row(platform, recipe.runs.get(platform)).trim_end()
            );
        }
    }
    Ok(())
}
