//! Runner and dispatch for `stado fleet`: it turns the parsed command tree
//! into one implementation call and translates the fleet's verdict into the
//! CLI's exit contract.

use crate::cli::fleet::{doctor, enroll, fleets, ingress, invite, key, ops};
use crate::cli::{CmdError, CLICK_ERROR_CODE};

use super::{FleetCommands, IngressCommands, KeyCommands};

/// Run one fleet command.
///
/// The commands report in the fleet's own vocabulary — `Ok(true)` is "done",
/// `Ok(false)` is "ran, and the fleet is not healthy" (only `doctor` says
/// that), `Err` is a failure with a sentence for the operator. The exit
/// contract is the CLI's: a verdict of `false` exits non-zero in silence,
/// because `doctor` already printed the failing rows and a second, classified
/// diagnosis line would contradict a command that deliberately said its own
/// last word.
pub async fn run(command: FleetCommands) -> Result<(), CmdError> {
    let outcome = execute(command).await;
    match outcome {
        Ok(true) => Ok(()),
        Ok(false) => Err(CmdError::silent(CLICK_ERROR_CODE)),
        Err(message) => Err(CmdError::click(message)),
    }
}

/// Dispatch to the implementation of one command, keeping the fleet's
/// `Result<bool, String>` verdict intact for [`run`] to translate.
async fn execute(command: FleetCommands) -> Result<bool, String> {
    match command {
        FleetCommands::Doctor { json, fleet } => doctor::run(json, fleet.as_deref()).await,
        FleetCommands::List { json } => fleets::list(json).await,
        FleetCommands::Status { name } => fleets::status(&name).await,
        FleetCommands::Create { name, notes } => ops::create(&name, &notes).await,
        FleetCommands::Assign { target, fleet } => ops::assign(&target, &fleet).await,
        FleetCommands::Delete { name } => ops::delete(&name).await,
        FleetCommands::Enroll {
            name,
            ssh,
            kind,
            fleet,
            bootstrap,
            install_key,
        } => {
            ops::enroll(
                &name,
                Some(&ssh),
                &kind,
                fleet.as_deref(),
                bootstrap,
                install_key,
            )
            .await
        }
        FleetCommands::Invite {
            name,
            expires,
            uses,
            offline,
            json,
        } => invite::invite(name.as_deref(), &expires, uses, offline, json).await,
        FleetCommands::Invites { json } => invite::invites(json).await,
        FleetCommands::RevokeInvite { id } => invite::revoke_invite(&id).await,
        FleetCommands::Ingress(sub) => match sub {
            IngressCommands::Up { port, named } => ingress::up(port, named).await,
            IngressCommands::Status { json } => ingress::status(json).await,
            IngressCommands::Down => ingress::down().await,
        },
        FleetCommands::Methods { json } => enroll::catalog::methods(json).await,
        FleetCommands::Join => enroll::join().await,
        FleetCommands::Pending { json } => enroll::pending(json).await,
        FleetCommands::Approve { hostname, fleet } => {
            enroll::approve(&hostname, fleet.as_deref()).await
        }
        FleetCommands::Reject { hostname } => enroll::reject(&hostname).await,
        FleetCommands::Catalog { json } => enroll::catalog::catalog(json).await,
        FleetCommands::Key(sub) => {
            let runner = crate::deploy::production_runner();
            match sub {
                KeyCommands::Add { target, from } => key::add(&runner, &target, &from).await,
                KeyCommands::Ls => key::ls().await,
                KeyCommands::Rm { target } => key::rm(&target).await,
                KeyCommands::Install { target } => key::install(&runner, &target).await,
                KeyCommands::Check { target } => key::check(&runner, &target).await,
                KeyCommands::Generate { target } => key::rotate::generate(&runner, &target).await,
                KeyCommands::Rotate { target } => key::rotate::rotate(&runner, &target).await,
            }
        }
    }
}
