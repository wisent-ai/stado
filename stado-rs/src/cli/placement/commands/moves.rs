//! `stado placement move` — the operator half: resolve the profile, claim the
//! transaction through registry CAS on the host the command runs on, then
//! print the receipt or the refusal with its rollback detail.
//!
//! The transaction itself is [`relocate`], kept apart from the command so the
//! claim, the execution and the rollback stay one set of rules.

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use crate::cli::placement::candidates::{
    declared_profile_hosts, ensure_profile_lifecycle_mutable, parse_registry, profile_host, target,
};
use crate::cli::placement::transfer::{
    execute_move, forget_progress, production_committer, release_claim, rollback,
    settle_state_backup, MoveContext, Progress, StepTiming,
};
use crate::cli::{registry, CmdError};
use crate::deploy::production_runner;
use crate::placement::{self, PlacementProfile, PlacementTransaction};
use crate::targets::Registry;

/// Which host a profile is placed on. `Err` names the refusal a move would
/// meet before touching any host.
pub(crate) fn placed_host(
    registry: &Registry,
    profile: &PlacementProfile,
) -> Result<String, String> {
    let sources = declared_profile_hosts(registry, profile).map_err(|error| error.to_string())?;
    match sources.as_slice() {
        [source] => Ok(source.clone()),
        [] => Err(format!(
            "placement profile {:?} has no complete managed source",
            profile.name
        )),
        _ => Err(format!(
            "placement profile {:?} is active on multiple hosts: {}",
            profile.name,
            sources.join(", ")
        )),
    }
}

/// What one relocation left behind.
pub(super) struct MoveReceipt {
    status: &'static str,
    transaction_id: Option<String>,
    profile: String,
    from_host: String,
    to_host: String,
    registry_generation: Option<String>,
    /// Each file or tree the destination held at a state path before the
    /// move, kept beside the moved state rather than deleted.
    kept_replaced_state: Vec<String>,
    /// Each state whose backup could not be settled, with why; its backup
    /// stays where the transaction put it.
    unsettled_state: Vec<String>,
    /// Every step the move ran, with its host and time, so a move that ran
    /// long says where the time went.
    steps: Vec<StepTiming>,
}

impl MoveReceipt {
    /// The receipt as `placement move --json` prints it.
    pub(super) fn report(&self) -> Value {
        json!({
            "status": self.status,
            "transaction_id": self.transaction_id,
            "profile": self.profile,
            "from_host": self.from_host,
            "to_host": self.to_host,
            "registry_generation": self.registry_generation,
            "kept_replaced_state": self.kept_replaced_state,
            "unsettled_state": self.unsettled_state,
            "steps": self.steps,
        })
    }
}

/// The steps of a move as one line each: what, where, how long, and
/// whether it ended with its answer.
fn timeline(steps: &[StepTiming]) -> Vec<String> {
    steps
        .iter()
        .map(|step| {
            format!(
                "{} on {}: {:.1}s{}",
                step.step,
                step.host,
                step.seconds,
                if step.succeeded { "" } else { " (failed)" }
            )
        })
        .collect()
}

/// Move `profile` to `to_host` from where it is placed now, on this host.
///
/// Every host reached by the transaction — the source, the destination —
/// is reached through its own channel from the host this runs on, and the
/// registry is written through the same compare-and-swap every registry
/// writer uses, so the move needs no other host to run it. `document` and
/// `generation` are the registry read the move starts from; the claim is
/// compare-and-swapped against that generation, so a document that changed
/// underneath is a refusal, never a move over someone else's commit.
pub(super) async fn relocate(
    document: Value,
    generation: String,
    profile: PlacementProfile,
    to_host: &str,
) -> Result<MoveReceipt, CmdError> {
    let parsed_registry = parse_registry(&document)?;
    profile_host(&profile, to_host)?;
    let destination = target(&parsed_registry, to_host)?.clone();
    let source_name = placed_host(&parsed_registry, &profile).map_err(CmdError::declaration)?;
    if source_name == to_host {
        return Ok(MoveReceipt {
            status: "already_placed",
            transaction_id: None,
            profile: profile.name,
            from_host: source_name,
            to_host: to_host.to_string(),
            registry_generation: None,
            kept_replaced_state: Vec::new(),
            unsettled_state: Vec::new(),
            steps: Vec::new(),
        });
    }
    let source = target(&parsed_registry, &source_name)?.clone();
    let transaction = PlacementTransaction {
        id: uuid::Uuid::new_v4().to_string(),
        profile: profile.name.clone(),
        from_host: source.name.clone(),
        to_host: destination.name.clone(),
        started_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
    };
    let mut claimed_document = document;
    // The document parsed as a registry above, so the one way the claim is
    // refused is another transaction already holding it.
    placement::claim_transaction(&mut claimed_document, &transaction).map_err(CmdError::refused)?;
    // The claim is a registry compare-and-swap, which on a host whose
    // registry authority is another host runs on that authority: said as its
    // own wait, so a move standing before its first step names the claim.
    let claim_generation = crate::wait::until(
        crate::wait::Kind::Host,
        format!(
            "placement move {} of {}: claim the transaction",
            transaction.id, transaction.profile
        ),
        "the registry",
        registry::push_document_if(&claimed_document, &generation),
    )
    .await?;
    let context = MoveContext {
        profile,
        source,
        destination,
        registry: parsed_registry,
        claimed_document,
        claim_generation,
        transaction,
    };
    let runner = production_runner();
    let committer = production_committer();
    let mut progress = Progress::default();
    match execute_move(&context, &mut progress, &runner, &committer).await {
        Ok(committed_generation) => {
            // The commit ends the transaction: no release will read how far
            // it got.
            forget_progress(&context.transaction.id);
            // Each state's backup is its own file on the destination, so all
            // are settled at once; the receipt keeps the profile's order.
            let mut kept_replaced_state = Vec::new();
            let mut unsettled_state = Vec::new();
            let settles = progress.destination_written.iter().map(|state| {
                settle_state_backup(
                    &context.destination,
                    state,
                    &context.transaction.id,
                    &runner,
                )
            });
            let settled = futures::future::join_all(settles).await;
            for (state, settled) in progress.destination_written.iter().zip(settled) {
                match settled {
                    Ok(Some(kept)) => kept_replaced_state.push(kept),
                    Ok(None) => {}
                    Err(error) => unsettled_state.push(format!("{}: {error}", state.path)),
                }
            }
            Ok(MoveReceipt {
                status: "moved",
                transaction_id: Some(context.transaction.id),
                profile: context.profile.name,
                from_host: context.source.name,
                to_host: context.destination.name,
                registry_generation: Some(committed_generation),
                kept_replaced_state,
                unsettled_state,
                steps: progress.steps,
            })
        }
        // The move's own failure is the class the caller acts on; what the
        // rollback and the claim release did is said beside it.
        Err(primary) => {
            let rollback_errors = rollback(&context, &progress, &runner).await;
            let release_error = release_claim(&context.transaction.id).await.err();
            // A claim still held is a transaction `placement release` will
            // end; its progress stays for it to read. A released claim has no
            // reader left.
            if release_error.is_none() {
                forget_progress(&context.transaction.id);
            }
            let mut failure = primary;
            if !progress.steps.is_empty() {
                failure = failure.also(format!(
                    "steps the move ran: {}",
                    timeline(&progress.steps).join("; ")
                ));
            }
            if !rollback_errors.is_empty() {
                failure =
                    failure.also(format!("rollback failures: {}", rollback_errors.join("; ")));
            }
            if let Some(error) = release_error {
                failure = failure.also(format!("registry lock release failed: {error}"));
            }
            Err(failure)
        }
    }
}

pub(super) async fn move_services(
    requested: &[String],
    to_host: &str,
    check: bool,
    vault_owner_rotated: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let (document, generation) = registry::fetch_versioned_document().await?;
    crate::targets::validate_registry(&document).map_err(CmdError::from)?;
    let profile =
        placement::profile_for_services(&document, requested).map_err(|error| match error {
            placement::ProfileLookupError::Request(detail) => CmdError::usage(detail),
            placement::ProfileLookupError::NoMatch(detail) => CmdError::missing(detail),
            placement::ProfileLookupError::Declaration(detail) => CmdError::declaration(detail),
        })?;
    ensure_profile_lifecycle_mutable(&profile)?;
    if check {
        return super::check::check_move(document, generation, profile, to_host, json_output).await;
    }
    // The vault's ciphertext moves with its profile and its owner's secret
    // key does not: moved before the owner is rotated to a key the
    // destination holds, it lands where no key opens it.
    let vault = crate::cli::secrets::VAULT_SERVICE;
    if !vault_owner_rotated && profile.services.iter().any(|service| service == vault) {
        return Err(CmdError::refused(format!(
            "placement profile {} carries {vault}, the fleet vault, whose owner key does not move \
             with it; move it with stado credentials vault move --to {to_host} --owner-uid <uid> \
             --apply, which rotates the owner to a key {to_host} holds and then moves the profile",
            profile.name
        )));
    }
    let moved = relocate(document, generation, profile, to_host).await;
    let kept = moved.as_ref().map(MoveReceipt::report);
    crate::cli::placement::history::record("move", kept.as_ref().map_err(|error| *error));
    let receipt = moved?;
    let report = receipt.report();
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    match &receipt.registry_generation {
        None => println!(
            "{} is already placed on {}",
            receipt.profile, receipt.to_host
        ),
        Some(generation) => println!(
            "moved {} from {} to {} (registry generation {generation})",
            receipt.profile, receipt.from_host, receipt.to_host
        ),
    }
    for kept in &receipt.kept_replaced_state {
        println!(
            "  kept what {} held there before the move: {kept}",
            receipt.to_host
        );
    }
    for unsettled in &receipt.unsettled_state {
        eprintln!(
            "  {}: the backup of {unsettled}; it stays under its .pre-stado-placement- name",
            receipt.to_host
        );
    }
    for line in timeline(&receipt.steps) {
        println!("  {line}");
    }
    Ok(())
}
