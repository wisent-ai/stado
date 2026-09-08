//! `service catalog`, and the two repairs that assert a unit from inside the
//! binary rather than from an operator's command line.

use super::*;

use super::super::declare::ensure::run::ensure;
use super::super::declare::ensure::EnsureOptions;

/// `service catalog`: the preconfigured Wisent services this build ships,
/// printed as they would deploy. Read-only and local: the answer comes from
/// the compiled-in document, never from a host.
pub(crate) async fn catalog(json: bool) -> Result<(), CmdError> {
    let entries = crate::deploy::service_catalog::all()
        .map_err(|error| CmdError::click(error.to_string()))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "services": entries.iter().map(|entry| json!({
                    "name": entry.name,
                    "summary": entry.summary,
                    "program": entry.program,
                    "args": entry.args,
                })).collect::<Vec<_>>(),
            }))?
        );
    } else {
        for entry in &entries {
            println!(
                "{:<14} {} {}",
                entry.name,
                entry.program,
                entry.args.join(" ")
            );
            println!("{:<14} {}", "", entry.summary);
        }
    }
    Ok(())
}

/// Ensure one dependency on the machine running this CLI.
///
/// Release submission uses this before its first object write. Keeping the
/// call inside the binary means every caller gets the same repair; a workflow
/// cannot accidentally rely on a listener left behind by an earlier run.
pub(crate) async fn ensure_local_dependency(
    name: &str,
    reason: &str,
    as_daemon: bool,
) -> Result<(), CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let registry = crate::cli::registry::read_registry().await?;
    let host = registry
        .lookup_self(&hostname)
        .map_err(|error| CmdError::click(error.to_string()))?
        .map(|target| target.name.clone())
        .ok_or_else(|| {
            CmdError::click(format!(
                "cannot ensure {name}: this machine {hostname:?} is not a registry target"
            ))
        })?;
    ensure(EnsureOptions {
        name,
        host: &host,
        from: None,
        args: &[],
        env: &[],
        reason,
        as_daemon,
        as_launch_agent: false,
        as_json: false,
    })
    .await
}

/// Reload an existing managed service after its configuration changes.
///
/// Configuration values are cached for the process lifetime. Native restart
/// preserves the unit's current command, including adopted units that have no
/// catalog recipe from which `ensure` could rebuild them.
pub(crate) async fn reconcile_after_config_change(name: &str, host: &str) -> Result<(), CmdError> {
    restart(name, Some(host), None, None, false).await
}
