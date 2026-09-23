//! Execution for `stado-migrate coordinator` plans.
//!
//! Every remote action goes through Stado's own deploy machinery: the
//! production runner plus the same argv builders `stado bootstrap` uses.
//! The registry flip goes through the validated compare-and-swap write
//! path shared with `stado registry push`, never a hand-rolled upload.

use serde_json::Value;
use stado::cli::registry::{fetch_document, push_document};
use stado::config;
use stado::deploy::bootstrap::{install_spec, ssh_argv};
use stado::deploy::{production_runner, CommandSpec, Runner};

use crate::plan::{build, MigrationPlan};

/// Label prefix every Stado coordinator service registers under (launchd).
const COORDINATOR_LABEL_PREFIX: &str = "com.wisent.compute.coordinator.";
/// Archive name used to carry a device-local store to the target host.
const STORE_ARCHIVE: &str = "stado-migrate-store.tgz";

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

async fn local_uid(runner: &Runner) -> Result<String, String> {
    let out = run_checked(
        runner,
        CommandSpec::new(vec!["id".to_string(), "-u".to_string()]),
        "id -u",
    )
    .await?;
    Ok(out.trim().to_string())
}

/// Stop the source daemon. A locally registered service is booted out and
/// confirmed gone; a source entry with a remote destination is stopped
/// through the deploy channel. A service that is not loaded anywhere is
/// reported and treated as already stopped.
async fn stop_source(runner: &Runner, plan: &MigrationPlan) -> Result<(), String> {
    let label = label(&plan.from_name);
    let uid = local_uid(runner).await?;
    let print_spec = CommandSpec::new(vec![
        "launchctl".to_string(),
        "print".to_string(),
        format!("gui/{uid}/{label}"),
    ]);
    if runner(print_spec.clone()).await?.ok() {
        run_checked(
            runner,
            CommandSpec::new(vec![
                "launchctl".to_string(),
                "bootout".to_string(),
                format!("gui/{uid}/{label}"),
            ]),
            "launchctl bootout",
        )
        .await?;
        if runner(print_spec).await?.ok() {
            return Err(format!("service {label} is still loaded after bootout"));
        }
        println!("[stop] {label} booted out locally");
        return Ok(());
    }
    match plan.from_host.as_deref() {
        Some(host) if !host.contains("://") => {
            let print_cmd = format!("launchctl print gui/$(id -u)/{label}");
            let remote = runner(CommandSpec::new(ssh_argv(host, &print_cmd))).await?;
            if remote.ok() {
                let boot_cmd = format!("launchctl bootout gui/$(id -u)/{label}");
                run_checked(
                    runner,
                    CommandSpec::new(ssh_argv(host, &boot_cmd)),
                    "remote launchctl bootout",
                )
                .await?;
                println!("[stop] {label} booted out on {host}");
            } else {
                println!("[stop] {label} is not loaded on {host}; nothing to stop");
            }
            Ok(())
        }
        _ => {
            println!("[stop] {label} is not loaded locally; nothing to stop");
            Ok(())
        }
    }
}

fn expand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => format!("{}/{rest}", home.to_string_lossy()),
            None => path.to_string(),
        },
        None => path.to_string(),
    }
}

/// Carry the device-local queue store to the target host as a tarball.
/// The target's own config decides whether it reads this path; the plan
/// text says so.
async fn move_store(runner: &Runner, plan: &MigrationPlan) -> Result<(), String> {
    let store = expand_home(config::wc_local_storage_path());
    let archive = std::env::temp_dir().join(STORE_ARCHIVE);
    let archive_str = archive.to_string_lossy().to_string();
    let parent = std::path::Path::new(&store)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| format!("store path {store} has no parent directory"))?;
    let base = std::path::Path::new(&store)
        .file_name()
        .map(|b| b.to_string_lossy().to_string())
        .ok_or_else(|| format!("store path {store} has no final component"))?;
    run_checked(
        runner,
        CommandSpec::new(vec![
            "tar".to_string(),
            "-czf".to_string(),
            archive_str.clone(),
            "-C".to_string(),
            parent,
            base,
        ]),
        "store archive",
    )
    .await?;
    run_checked(
        runner,
        CommandSpec::new(vec![
            "scp".to_string(),
            archive_str.clone(),
            format!("{}:.stado/{STORE_ARCHIVE}", plan.to_host),
        ]),
        "store upload",
    )
    .await?;
    let untar = format!(
        "mkdir -p \"$HOME/.stado\" && tar -xzf \"$HOME/.stado/{STORE_ARCHIVE}\" -C \"$HOME/.stado\" && rm \"$HOME/.stado/{STORE_ARCHIVE}\""
    );
    run_checked(
        runner,
        CommandSpec::new(ssh_argv(&plan.to_host, &untar)),
        "store unpack on target",
    )
    .await?;
    let _ = std::fs::remove_file(&archive);
    println!("[store] device-local store copied to {}", plan.to_host);
    Ok(())
}

/// Install and start the coordinator on the target through the same
/// `stado bootstrap --local --target` path `install_macos_coordinator.sh`
/// wraps. The remote daemon starts with its entry name, so its survival
/// check passes as long as the entry exists in the registry.
async fn bootstrap_target(runner: &Runner, plan: &MigrationPlan) -> Result<(), String> {
    let script = format!(
        "STADO_BIN=\"$HOME/.stado/bin/stado\"; [ -x \"$STADO_BIN\" ] || STADO_BIN=\"$(command -v stado)\"; exec \"$STADO_BIN\" bootstrap --local --target '{name}'",
        name = plan.to_name
    );
    run_checked(
        runner,
        CommandSpec::new(ssh_argv(&plan.to_host, &script)),
        "remote coordinator bootstrap",
    )
    .await?;
    println!(
        "[bootstrap] coordinator '{}' installed on {}",
        plan.to_name, plan.to_host
    );
    Ok(())
}

/// The single atomic registry mutation of the whole migration: one
/// compare-and-swapped document where exactly the target is active.
async fn flip_registry(plan: &MigrationPlan) -> Result<String, String> {
    let mut document = fetch_document().await.map_err(|exc| exc.to_string())?;
    let entries = document
        .get_mut("coordinators")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "registry document has no coordinators array".to_string())?;
    for entry in entries.iter_mut() {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        entry["active"] = Value::Bool(name == plan.to_name);
    }
    let generation = push_document(&document)
        .await
        .map_err(|exc| exc.to_string())?;
    println!(
        "[registry] active moved '{}' -> '{}' (generation {generation})",
        plan.from_name, plan.to_name
    );
    Ok(generation)
}

/// Read the registry back and require the target as the only active entry;
/// then report the remote service state without enforcing it, since launchd
/// visibility can lag the bootstrap by a moment.
async fn verify(runner: &Runner, plan: &MigrationPlan) -> Result<(), String> {
    let document = fetch_document().await.map_err(|exc| exc.to_string())?;
    let active: Vec<String> = document
        .get("coordinators")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| entry.get("active").and_then(Value::as_bool) == Some(true))
                .filter_map(|entry| {
                    entry
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    if active != vec![plan.to_name.clone()] {
        return Err(format!(
            "registry verification failed: active coordinators are now: {}",
            active.join(", ")
        ));
    }
    let check = format!("launchctl print gui/$(id -u)/{}", label(&plan.to_name));
    let out = runner(CommandSpec::new(ssh_argv(&plan.to_host, &check))).await?;
    if out.ok() {
        println!(
            "[verify] {} reports the coordinator service loaded",
            plan.to_host
        );
    } else {
        println!(
            "[verify] warning: service not visible on {} yet: {}",
            plan.to_host,
            out.detail()
        );
    }
    Ok(())
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
