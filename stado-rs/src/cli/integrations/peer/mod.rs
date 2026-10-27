//! `stado market --provider stado` and `stado market peer`: trading this
//! fleet's idle capacity with another Stado fleet.
//!
//! - [`declare`]: `peer add|edit|remove|list|show`, the peer declarations.
//! - [`offers`]: `list`, `unlist`, `status` and `monitor` against one peer.
//! - [`readiness`]: whether this host can lend to the peer at all.
//! - [`run`]: the `follow` loop that lends this host while our queue is
//!   idle.
//!
//! The provider is in [`crate::providers::peer`]; this layer turns its
//! answers into the command's output and its refusals into exit codes.

mod declare;
mod offers;
mod readiness;
mod run;

use crate::cli::{CmdError, MarketCommands};
use crate::primitives::failure::FailureCode;
use crate::providers::peer::PeerError;
use crate::queue::JobStorage;

pub(crate) use declare::declare;

/// A peer failure as the command reports it, with its class.
pub(crate) fn cmd(error: PeerError) -> CmdError {
    match error {
        PeerError::Usage(message) => CmdError::usage(message),
        PeerError::NotFound(message) => CmdError::missing(message),
        PeerError::Refused(message) => CmdError::refused(message),
        PeerError::Credential(message) => CmdError::click(message).stating(FailureCode::Config),
        PeerError::Storage(error) => CmdError::from(error),
        PeerError::Local(message) => CmdError::declaration(message),
    }
}

/// This fleet's own store, where peers and offers are declared.
pub(crate) async fn our_store() -> Result<JobStorage, CmdError> {
    JobStorage::new().await.map_err(CmdError::from)
}

/// The `--peer` a provider-stado subcommand needs, refused when absent.
fn required_peer(peer: &Option<String>, verb: &str) -> Result<String, CmdError> {
    match peer {
        Some(peer) => Ok(peer.clone()),
        None => Err(CmdError::usage(format!(
            "stado market {verb} --provider stado needs --peer NAME, the declared peer fleet; \
             `stado market peer list` names them"
        ))),
    }
}

/// Dispatch one provider-stado subcommand.
pub(crate) async fn dispatch(command: &MarketCommands) -> Result<(), CmdError> {
    match command {
        MarketCommands::List {
            peer,
            price_gpu,
            price_disk,
            price_min_bid,
            json,
            ..
        } => {
            if price_disk.is_some() || price_min_bid.is_some() {
                return Err(CmdError::usage(
                    "--price-disk and --price-min-bid are Vast.ai's prices; a peer fleet is \
                     offered at --price-gpu alone",
                ));
            }
            offers::list(&required_peer(peer, "list")?, *price_gpu, *json).await
        }
        MarketCommands::Unlist { peer, json, .. } => {
            offers::unlist(&required_peer(peer, "unlist")?, *json).await
        }
        MarketCommands::Status { peer, json, .. } => offers::status(peer.as_deref(), *json).await,
        MarketCommands::Readiness {
            peer,
            vault_host,
            no_vault_check,
            json,
            ..
        } => {
            readiness::report(
                &required_peer(peer, "readiness")?,
                vault_host.clone(),
                !*no_vault_check,
                *json,
            )
            .await
        }
        MarketCommands::Monitor {
            peer, bucket, json, ..
        } => {
            let bucket = match bucket.as_deref() {
                Some(named) => named,
                None => crate::config::bucket(),
            };
            offers::monitor(&required_peer(peer, "monitor")?, bucket, *json).await
        }
        MarketCommands::Follow {
            peer,
            idle_window_s,
            poll_interval_s,
            price_gpu,
            price_disk,
            max_duration_s,
            dry_run,
            once,
            ..
        } => {
            if price_disk.is_some() {
                return Err(CmdError::usage(
                    "--price-disk is Vast.ai's price; a peer fleet is offered at --price-gpu alone",
                ));
            }
            run::auto_list(run::Params {
                peer: required_peer(peer, "follow")?,
                idle_window_s: *idle_window_s,
                poll_interval_s: *poll_interval_s,
                price_gpu: *price_gpu,
                max_duration_s: *max_duration_s,
                dry_run: *dry_run,
                once: *once,
            })
            .await
        }
        MarketCommands::Peer(peer) => declare(peer).await,
    }
}
