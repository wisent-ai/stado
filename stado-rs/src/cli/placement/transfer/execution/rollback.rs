//! The reverse of every step a move took, and the release of its claim: what
//! a failed move, and `stado placement release`, do to put the profile back
//! where the directory still places it.

use super::super::readiness::apply_routes;
use super::super::state::restore_state;
use super::super::units::{act_on_unit, UnitAction};
use super::super::{MoveContext, Progress};
use crate::cli::placement::candidates::{profile_host, unit};
use crate::cli::{registry, CmdError};
use crate::deploy::Runner;
use crate::placement;

/// Release the placement claim `transaction_id` holds. Releasing is a pure
/// transform of the registry document, so it goes through the registry's
/// commit, which reapplies it only after a lost race and answers every other
/// failure as it is; a claim already gone changes nothing.
pub(in crate::cli::placement) async fn release_claim(transaction_id: &str) -> Result<(), CmdError> {
    registry::commit_document(|current| {
        let mut next = current.clone();
        placement::release_transaction(&mut next, transaction_id).map_err(CmdError::declaration)?;
        Ok(next)
    })
    .await?;
    Ok(())
}

pub(in crate::cli::placement) async fn rollback(
    context: &MoveContext,
    progress: &Progress,
    runner: &Runner,
) -> Vec<String> {
    let mut errors = Vec::new();
    let destination_profile = match profile_host(&context.profile, &context.destination.name) {
        Ok(profile) => profile,
        Err(error) => {
            errors.push(error.to_string());
            return errors;
        }
    };
    let source_profile = match profile_host(&context.profile, &context.source.name) {
        Ok(profile) => profile,
        Err(error) => {
            errors.push(error.to_string());
            return errors;
        }
    };

    if progress.destination_started {
        for logical in &context.profile.stop_order {
            match unit(destination_profile, logical) {
                Ok(spec) => {
                    if let Err(error) =
                        act_on_unit(&context.destination, spec, UnitAction::Retire, runner).await
                    {
                        errors.push(error.to_string());
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
    }
    // Each written state is its own file on the destination, so all are put
    // back at once: the source stays fenced until the rollback ends, and the
    // service is down for the longest restore, not their sum. Every failure
    // is kept, in the reverse of the order the states were written.
    let restores =
        progress.destination_written.iter().rev().map(|state| {
            restore_state(&context.destination, state, &context.transaction.id, runner)
        });
    for restored in futures::future::join_all(restores).await {
        if let Err(error) = restored {
            errors.push(error.to_string());
        }
    }
    if progress.route_applied {
        if let Err(error) = apply_routes(context, &context.source.name, runner).await {
            errors.push(error.to_string());
        }
    }
    if progress.source_stopped || progress.source_retired {
        for logical in &context.profile.start_order {
            match unit(source_profile, logical) {
                Ok(spec) => {
                    if let Err(error) =
                        act_on_unit(&context.source, spec, UnitAction::Start, runner).await
                    {
                        errors.push(error.to_string());
                    }
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
    }
    errors
}
