//! `stado placement release ID`: end a placement move the registry still
//! holds open after the process running it ended without committing or rolling
//! back, which leaves every service of its profile refused to every consumer.
//!
//! The release does what that move's own failure would have done: the
//! rollback of every step it took, then the release of its claim. A move
//! keeps how far it got on the host running it
//! (`~/.stado/placement/<transaction>.json`); a release there undoes exactly
//! those steps. A release on another host, or one that finds no record,
//! counts every step as taken and says so, and each undo ends where it
//! should on a step that never ran. The directory still places the profile on
//! the move's source, because only the move's commit retargets it, so the
//! rollback returns the services to where consumers are already sent. A move
//! still running when its claim is released finds the registry changed at its
//! own commit and rolls back too.

use serde_json::json;

use super::candidates::{parse_registry, target};
use super::transfer::{
    forget_progress, recorded_progress, release_claim, rollback, running_move_pid, MoveContext,
    Progress,
};
use crate::cli::{registry, CmdError};
use crate::deploy::production_runner;
use crate::placement;

pub(crate) async fn release(id: &str, json_output: bool) -> Result<(), CmdError> {
    let (document, generation) = registry::fetch_versioned_document().await?;
    let open = placement::transactions(&document).map_err(CmdError::declaration)?;
    let Some(transaction) = open.into_iter().find(|transaction| transaction.id == id) else {
        return Err(CmdError::missing(format!(
            "no placement transaction {id} is open; stado placement transactions lists the open ones"
        )));
    };
    // A move whose own process still runs on this host commits or rolls back
    // itself; releasing it under that process would run two rollbacks of one
    // transaction against the same hosts at once.
    match running_move_pid(id) {
        Ok(Some(pid)) => {
            return Err(CmdError::refused(format!(
                "placement transaction {id} is still run by process {pid} on this host, which \
                 commits or rolls it back itself; release it only once that process has ended \
                 (stado placement transactions shows whether it still runs)"
            )))
        }
        Ok(None) => {}
        Err(cause) => eprintln!(
            "stado: whether a process on this host still runs placement transaction {id} cannot \
             be read, so it is released: {cause}"
        ),
    }
    let profile = placement::profiles(&document)
        .map_err(CmdError::declaration)?
        .into_iter()
        .find(|profile| profile.name == transaction.profile)
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "placement transaction {id} holds placement profile {}, which the registry no longer declares, so its units cannot be named",
                transaction.profile
            ))
        })?;
    let parsed_registry = parse_registry(&document)?;
    let source = target(&parsed_registry, &transaction.from_host)?.clone();
    let destination = target(&parsed_registry, &transaction.to_host)?.clone();
    let (progress, progress_source) = match recorded_progress(id, &profile) {
        Ok(Some(progress)) => (progress, "the steps this host recorded the move taking"),
        Ok(None) => (
            Progress::every_step(&profile),
            "every step, since this host keeps no record of how far the move got",
        ),
        Err(cause) => {
            eprintln!(
                "stado: the progress this host recorded for placement transaction {id} cannot be \
                 read, so every step is undone: {cause}"
            );
            (
                Progress::every_step(&profile),
                "every step, since this host's record of the move cannot be read",
            )
        }
    };
    let context = MoveContext {
        profile,
        source,
        destination,
        registry: parsed_registry,
        claimed_document: document,
        claim_generation: generation,
        transaction,
    };
    // What the release is about to do is said on stderr, so standard output
    // carries only its answer: the JSON report with --json.
    eprintln!(
        "releasing placement transaction {id}: restoring {} on {} and retiring it on {}, undoing {progress_source}",
        context.profile.name, context.source.name, context.destination.name
    );
    let rollback_errors = rollback(&context, &progress, &production_runner()).await;
    release_claim(id).await?;
    forget_progress(id);
    let report = json!({
        "status": "released",
        "transaction_id": id,
        "profile": context.profile.name,
        "from_host": context.source.name,
        "to_host": context.destination.name,
        "rollback_failures": rollback_errors,
        "undid": progress_source,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "released placement transaction {id}: {} resolves again on {}",
            context.profile.name, context.source.name
        );
        for failure in &rollback_errors {
            eprintln!("  rollback step failed: {failure}");
        }
    }
    match rollback_errors.is_empty() {
        true => Ok(()),
        false => Err(CmdError::click(format!(
            "placement transaction {id} is released, and {} rollback step(s) failed: {}",
            rollback_errors.len(),
            rollback_errors.join("; ")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)),
    }
}
