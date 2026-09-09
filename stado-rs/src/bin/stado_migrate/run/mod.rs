//! Execution for `stado-migrate coordinator` plans.
//!
//! Every remote action goes through Stado's own deploy machinery: the
//! production runner plus the same argv builders `stado bootstrap` uses.
//! The registry flip goes through the validated compare-and-swap write
//! path shared with `stado registry push`, never a hand-rolled upload.

mod hosts;
mod registry;

use stado::cli::registry::fetch_document;
use stado::config;
use stado::deploy::bootstrap::install_spec;
use stado::deploy::{production_runner, CommandSpec, Runner};

use crate::plan::build;

use self::hosts::{bootstrap_target, move_store, stop_source};
use self::registry::{flip_registry, verify};

/// Label prefix every Stado coordinator service registers under (launchd).
const COORDINATOR_LABEL_PREFIX: &str = "com.wisent.compute.coordinator.";

fn label(name: &str) -> String {
    format!("{COORDINATOR_LABEL_PREFIX}{name}")
}

async fn run_checked(runner: &Runner, spec: CommandSpec, what: &str) -> Result<String, String> {
    let output = runner(spec).await?;
    if output.ok() {
        Ok(output.stdout)
    } else {
        Err(format!("{what} failed: {}", output.detail()))
    }
}

/// Plan and (unless dry-run) execute the full coordinator migration in an
/// order that never leaves two daemons ticking: preflight binaries, stop the
/// source, optionally carry the store, bootstrap the target, flip the
/// registry, verify.
pub async fn migrate_coordinator(
    to: &str,
    from: Option<&str>,
    dry_run: bool,
    move_local_storage: bool,
) -> Result<(), String> {
    let document = fetch_document().await.map_err(|exc| exc.to_string())?;
    let plan = build(
        &document,
        from,
        to,
        config::wc_storage_backend(),
        move_local_storage,
    )?;
    println!("migration plan ({} -> {}):", plan.from_name, plan.to_name);
    for step in &plan.steps {
        println!("  - {step}");
    }
    if dry_run {
        println!("dry-run: no changes made");
        return Ok(());
    }
    let runner = production_runner();
    run_checked(
        &runner,
        install_spec(&plan.to_host),
        "release binary install",
    )
    .await?;
    println!("[preflight] release binaries present on {}", plan.to_host);
    stop_source(&runner, &plan).await?;
    if plan.move_local_storage {
        move_store(&runner, &plan).await?;
    }
    bootstrap_target(&runner, &plan).await?;
    let generation = flip_registry(&plan).await?;
    verify(&runner, &plan).await?;
    println!(
        "migration complete: '{}' is the active coordinator (generation {generation})",
        plan.to_name
    );
    println!(
        "note: the previous daemon '{}' was stopped, not uninstalled; its service definition remains on the old host",
        plan.from_name
    );
    Ok(())
}
