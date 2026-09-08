//! The host probes: what one init system holds under a named unit, what it
//! is running that nothing declares, and who starts it.
//!
//! Every command here names the unit or the program it acts on. None of them
//! enumerates the registry, which is what makes them the ones that still
//! answer when the document is the thing that is wrong.

use super::*;

pub(crate) mod label_print;
pub(crate) mod reap;
pub(crate) mod watch_spawn;

/// `service bootout LABEL --host HOST [--domain system|user]` — take one exact
/// unit out of launchd or systemd, whether the registry declares it or not.
///
/// Without `--domain`, the system scope is tried first and the calling
/// account's scope only when the system manager holds no exact unit by that
/// name. Explicit `user` is what ends a stale user unit while leaving its
/// canonical system sibling running.
pub(crate) async fn bootout(
    label: &str,
    host: &str,
    domain: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let scope = service::BootoutScope::parse(domain).map_err(click)?;
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let runner = production_runner();
    let (state, detail) = service::bootout_label(&target, label, scope, &runner)
        .await
        .map_err(click)?;
    if json {
        return print_json(&json!({
            "host": target.name,
            "label": label,
            "state": state,
            "detail": detail,
        }));
    }
    table::print(
        &["HOST", "LABEL", "STATE", "DETAIL"],
        &[vec![
            target.name.clone(),
            label.to_string(),
            state.clone(),
            detail.clone(),
        ]],
    );
    if state == "refused" || state == "failed" {
        return Err(CmdError::click(format!(
            "{}: {label} {state}: {detail}",
            target.name
        )));
    }
    Ok(())
}
