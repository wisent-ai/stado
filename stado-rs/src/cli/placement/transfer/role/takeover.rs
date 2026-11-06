//! `stado placement role take SERVICE --reason WHY`, run on the host that
//! takes it: a role of the host Stado process (the fleet object API) taken
//! from an active host that does not answer.
//!
//! `role move` fences the source's store and copies it, so it needs the
//! source to answer. When the source is gone — switched off, asleep, off
//! the network — nothing can be fenced or copied, and every registry read
//! that goes through that store stands on the silent host: the registry
//! authority taken here with `placement move --authority` still reads its
//! registry from the old host's store, so the whole fleet waits on a machine
//! that will not answer.
//!
//! This is the role's counterpart of the authority takeover. This host must
//! hold the registry authority already (taken first with `placement move
//! --authority`), so the commit is its own. Its own object API — the store
//! its Stado serves from `--api-local-store` — becomes the route's active
//! endpoint; the registry, from this host's last-known-good copy, is written
//! into that store; this host's copy and every other answering host's copy
//! are handed the same registry. What the silent host's store held and this
//! host's does not is not recovered: the receipt says so, and names the
//! host, so whoever brings it back reads it before it serves again (its
//! answers then carry a lower registry generation and are refused).

use serde_json::{json, Value};

use super::route::{checked_route, served_address, switched};
use crate::cli::placement::candidates::{parse_registry, target};
use crate::cli::resolver::{
    current_target, install_authority, kept_last_good_document, AuthorityHandover,
};
use crate::cli::CmdError;

/// Take `service` on this host from an active host that does not answer.
pub(in crate::cli::placement) async fn take_role(
    service: &str,
    reason: &str,
    check: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let copy = kept_last_good_document()
        .map_err(CmdError::declaration)?
        .ok_or_else(|| {
            CmdError::refused(
                "this host keeps no last-known-good registry, so there is no registry to take the role with"
                    .to_string(),
            )
        })?;
    let here = current_target(&copy).map_err(CmdError::declaration)?;
    let authority = copy["service_directory"]["authority"]["target"].as_str();
    if authority != Some(here.as_str()) {
        return Err(CmdError::refused(format!(
            "{here} is not the registry authority by its copy (it names {}), so a role taken here would be \
             committed by no one; take the authority first: stado placement move --authority --to-host {here}",
            crate::cli::reporting::optional_text(authority)
        )));
    }
    let source = checked_route(&copy, service, &here)?;
    // A source that answers is moved with its store, not taken without it.
    let bound = answer_bound(&copy, &here)?;
    if let Some(version) = source_answers(bound).await {
        return Err(CmdError::refused(format!(
            "{source}, where {service} stands, answers (its store holds the registry at version {version}); \
             move the role with its store instead: stado placement role move {service} --to-host {here} \
             --reason <why>"
        )));
    }
    let parsed = parse_registry(&copy)?;
    let destination = target(&parsed, &here)?.clone();
    let served = served_address(&copy, &destination)?;
    super::carry::ensure_destination_store_answers(&served, &here).await?;
    let next = switched(&copy, service, &source, &here, &served)?;
    if check {
        return report(
            json_output,
            json!({
                "state": "would_take",
                "service": service,
                "from_host": source,
                "to_host": here,
                "endpoint": served,
                "reason": reason,
            }),
            format!(
                "{here} can take {service} from {source}, which does not answer: {here}'s own object API at \
                 {served} would serve it, with the registry from this host's copy; nothing was written"
            ),
        );
    }
    let content = serde_json::to_string_pretty(&next)?;
    let handover = AuthorityHandover {
        to_host: &here,
        old: &source,
        unanswered: Some(format!("{source} does not answer; {service} taken on {here}: {reason}")),
        store_at: Some(&served),
    };
    let installed = install_authority(&content, handover).await?;
    let receipt = json!({
        "state": "taken",
        "service": service,
        "from_host": source,
        "to_host": here,
        "endpoint": served,
        "reason": reason,
        "registry_generation": next["service_directory"]["generation"],
        "not_recovered": format!(
            "what {source}'s store held and {here}'s does not; read it there before {source} serves again"
        ),
        "install": installed.receipt,
    });
    crate::cli::placement::history::record("take-role", Ok(&receipt));
    if let Some(failure) = installed.failure {
        return Err(failure.also(format!("{service} was taken on {here}; receipt: {receipt}")));
    }
    report(
        json_output,
        receipt.clone(),
        format!(
            "{here} took {service} from {source}, which does not answer (registry generation {}); \
             what {source}'s store held and {here}'s does not was not recovered",
            crate::cli::reporting::field(&receipt["registry_generation"])
        ),
    )
}

/// How long the source's store is given to answer one registry read before
/// it is judged silent: the refresh interval the registry declares for this
/// host's resolver, the same bound every reader of the store stands.
fn answer_bound(copy: &Value, here: &str) -> Result<std::time::Duration, CmdError> {
    let config =
        crate::service_resolution::resolver_config(copy, here).map_err(CmdError::declaration)?;
    Ok(std::time::Duration::from_secs(config.refresh_seconds))
}

/// Whether the store this host reads its registry from — the route's active
/// host, through this host's adapter — answers one registry read within
/// `bound`; the version it holds when it does, `None` when it is silent.
async fn source_answers(bound: std::time::Duration) -> Option<String> {
    let url = crate::config::wc_stado_storage_url().ok()?;
    let store = crate::targets::RegistryStore::at_object_api(&url).ok()?;
    let read = tokio::time::timeout(bound, store.read_versioned()).await.ok()?;
    read.ok().flatten().map(|blob| blob.version)
}

fn report(json_output: bool, receipt: Value, line: String) -> Result<(), CmdError> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        println!("{line}");
    }
    Ok(())
}
