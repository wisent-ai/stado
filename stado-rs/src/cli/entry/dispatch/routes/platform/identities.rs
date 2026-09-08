//! Where the `stado identity` verbs land.

use crate::cli::*;

pub(super) async fn dispatch(command: IdentityCommands) -> Result<(), CmdError> {
    match command {
        IdentityCommands::List { json } => identity::list(json).await,
        IdentityCommands::Verify {
            kind,
            identity,
            json,
        } => identity::verify(kind, identity, json).await,
        IdentityCommands::RelayAppleChallenge {
            identity,
            authorization_id,
            preflight,
            json,
        } => identity::relay_apple_challenge(identity, authorization_id, preflight, json).await,
        IdentityCommands::IssueAppleCapabilities {
            target,
            agent,
            authorization_id,
            ttl_seconds,
            json,
        } => {
            identity::issue_apple_capabilities(target, agent, authorization_id, ttl_seconds, json)
                .await
        }
    }
}
