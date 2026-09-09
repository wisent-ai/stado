//! Host-side steps of a coordinator migration: stop the source daemon,
//! carry the device-local store, bootstrap the target.
//!
//! Every remote action goes through Stado's own deploy machinery: the
//! production runner plus the same argv builders `stado bootstrap` uses.

use stado::config;
use stado::deploy::bootstrap::ssh_argv;
use stado::deploy::{CommandSpec, Runner};

use crate::plan::MigrationPlan;

use super::{label, run_checked};

/// Archive name used to carry a device-local store to the target host.
const STORE_ARCHIVE: &str = "stado-migrate-store.tgz";

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
pub(super) async fn stop_source(runner: &Runner, plan: &MigrationPlan) -> Result<(), String> {
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
pub(super) async fn move_store(runner: &Runner, plan: &MigrationPlan) -> Result<(), String> {
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
pub(super) async fn bootstrap_target(runner: &Runner, plan: &MigrationPlan) -> Result<(), String> {
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
