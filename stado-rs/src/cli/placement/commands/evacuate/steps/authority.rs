//! The registry authority's steps of an evacuation: taken on B with
//! `placement move --authority`, and taken there first when the authority
//! does not answer.

use serde_json::Value;

use super::{run_self, starting};
use crate::cli::placement::commands::evacuate::scan;
use crate::cli::CmdError;
use crate::deploy::Runner;

/// `stado placement move --authority --to-host TO`, with `--check` unless
/// `apply`; this host must be TO, as the role is taken where it runs. When
/// the directory already names TO the authority (an evacuation run again
/// after a later step stopped it), there is nothing to take, and the step
/// says so instead of refusing the rerun.
pub(in crate::cli::placement::commands::evacuate) async fn authority(
    to: &str,
    apply: bool,
    runner: &Runner,
) -> Result<Value, CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    if scan::authority_holder(&document) == Some(to) {
        return Ok(serde_json::json!({ "status": "already_authority", "authority": to }));
    }
    let mut args = vec!["placement", "move", "--authority", "--to-host", to];
    if apply {
        starting(format!("taking the registry authority on {to}"));
    } else {
        args.push("--check");
    }
    let report = run_self(&args, runner).await?;
    // The steps after it read the registry this host now serves.
    if apply {
        crate::cli::resolver::follow_authority(false)
            .await
            .map_err(|error| error.also(format!("the registry authority itself was taken on {to}; the run stopped before the next step")))?;
    }
    Ok(report)
}

/// With `--authority` (run on TO): a registry authority that does not answer
/// commits no write, so it is taken on TO first and every other step goes
/// through TO; its receipt. A check cannot read through a silent authority,
/// so it refuses naming the silence; so does any run whose silent authority's
/// host also serves a role of the host Stado process, since the store a new
/// authority writes is that role's. `None` when the authority answers. Every
/// run starts here, so it also refuses an answering authority whose Stado
/// lacks `resolver follow`, which each moving step runs there.
pub(in crate::cli::placement::commands::evacuate) async fn authority_first(
    to: &str,
    asked: bool,
    check: bool,
) -> Result<Option<Value>, CmdError> {
    if !asked {
        crate::cli::resolver::ensure_upstream_follows().await?;
        return Ok(None);
    }
    crate::cli::placement::transfer::ensure_runs_on(to)?;
    let Err(silence) = crate::cli::resolver::authority_snapshot().await else {
        crate::cli::resolver::ensure_upstream_follows().await?;
        return Ok(None);
    };
    let copy = crate::cli::resolver::kept_last_good_document()
        .map_err(CmdError::declaration)?
        .ok_or_else(|| {
            CmdError::refused("this host keeps no registry copy naming the silent authority")
        })?;
    // This host's registry store is the fleet object API when its storage is
    // the object API; a takeover writes it, so a silent host serving it stops it.
    let served_store = crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
        == Some(crate::capabilities::StorageAdapter::StadoObject);
    if let Some(holder) = scan::authority_holder(&copy).filter(|_| served_store) {
        let roles = scan::fixed_routes_on(&copy, holder)?.roles;
        if !roles.is_empty() {
            return Err(CmdError::refused(format!(
                "the registry authority {holder} does not answer ({silence}) and also serves {}, the store a new \
                 authority would write, so neither can move without it; evacuate while {holder} answers",
                roles.join(", ")
            )));
        }
    }
    if check {
        return Err(CmdError::refused(format!(
            "the registry authority does not answer ({silence}), so no step can be checked through it; \
             run the evacuation without --check to take the authority on {to} first and check every other step through {to}"
        )));
    }
    authority(to, true, &crate::deploy::production_runner())
        .await
        .map(Some)
}
