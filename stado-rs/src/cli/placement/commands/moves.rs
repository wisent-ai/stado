//! `stado placement move` — the operator half: resolve the profile, decide
//! whether this host may run the transaction at all, claim it through registry
//! CAS, then print the receipt or the refusal with its rollback detail.
//!
//! The transaction itself is [`relocate`], kept apart from the command so the
//! claim, the execution and the rollback stay one set of rules.

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use super::delegate::delegate_to_registry_authority;
use crate::cli::placement::candidates::{
    declared_profile_hosts, ensure_profile_lifecycle_mutable, parse_registry, profile_host, target,
};
use crate::cli::placement::transfer::{
    execute_move, production_committer, release_claim, rollback, settle_state_backup, MoveContext,
    Progress,
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
struct MoveReceipt {
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
}

/// Move `profile` to `to_host` from where it is placed now, on this host.
///
/// The caller has already decided this host may run the transaction (it is
/// the authority, or the document has none). `document` and `generation` are
/// the registry read the decision was made on; the claim is compare-and-swapped
/// against that generation, so a document that changed underneath is a
/// refusal, never a move over someone else's commit.
async fn relocate(
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
    let claim_generation = registry::push_document_if(&claimed_document, &generation).await?;
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
            let mut kept_replaced_state = Vec::new();
            let mut unsettled_state = Vec::new();
            for state in &progress.destination_written {
                match settle_state_backup(
                    &context.destination,
                    state,
                    &context.transaction.id,
                    &runner,
                )
                .await
                {
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
            })
        }
        // The move's own failure is the class the caller acts on; what the
        // rollback and the claim release did is said beside it.
        Err(primary) => {
            let rollback_errors = rollback(&context, &progress, &runner).await;
            let release_error = release_claim(&context.transaction.id).await.err();
            let mut failure = primary;
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
    json_output: bool,
) -> Result<(), CmdError> {
    let (document, generation) = registry::fetch_versioned_document().await?;
    crate::targets::validate_registry(&document).map_err(CmdError::from)?;
    let parsed_registry = parse_registry(&document)?;
    let profile =
        placement::profile_for_services(&document, requested).map_err(|error| match error {
            placement::ProfileLookupError::Request(detail) => CmdError::usage(detail),
            placement::ProfileLookupError::NoMatch(detail) => CmdError::missing(detail),
            placement::ProfileLookupError::Declaration(detail) => CmdError::declaration(detail),
        })?;
    ensure_profile_lifecycle_mutable(&profile)?;
    if delegate_to_registry_authority(&document, &parsed_registry, requested, to_host, json_output)
        .await?
    {
        return Ok(());
    }
    let receipt = relocate(document, generation, profile, to_host).await?;
    let report = json!({
        "status": receipt.status,
        "transaction_id": receipt.transaction_id,
        "profile": receipt.profile,
        "from_host": receipt.from_host,
        "to_host": receipt.to_host,
        "registry_generation": receipt.registry_generation,
        "kept_replaced_state": receipt.kept_replaced_state,
        "unsettled_state": receipt.unsettled_state,
    });
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
    Ok(())
}
