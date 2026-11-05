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

use serde_json::{json, Value};

use super::market::{Follow, ListOffer, MarketAdapter, ServeOrRent};
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::providers::peer::PeerError;
use crate::queue::JobStorage;

pub(crate) use declare::declare;
pub(crate) use readiness::every_peer;

/// A peer failure as the command reports it, with its class.
pub(crate) fn cmd(error: PeerError) -> CmdError {
    match error {
        PeerError::Usage(message) => CmdError::usage(message),
        PeerError::NotFound(message) => CmdError::missing(message),
        PeerError::Refused(message) => CmdError::refused(message),
        PeerError::Credential(message, failure) => CmdError::click(message).stating(failure),
        PeerError::Storage(error) => CmdError::from(error),
        PeerError::Local(message) => CmdError::declaration(message),
    }
}

/// This fleet's own store, where peers and offers are declared.
pub(crate) async fn our_store() -> Result<JobStorage, CmdError> {
    JobStorage::new().await.map_err(CmdError::from)
}

/// Another Stado fleet, named by `--peer`: while our queue is idle this host
/// runs a Stado worker against the peer's queue.
pub(crate) struct PeerAdapter {
    pub peer: Option<String>,
}

impl PeerAdapter {
    /// The `--peer` a verb needs, refused when absent.
    fn required(&self, verb: &str) -> Result<String, CmdError> {
        match &self.peer {
            Some(peer) => Ok(peer.clone()),
            None => Err(CmdError::usage(format!(
                "stado market {verb} --provider stado needs --peer NAME, the declared peer fleet; \
                 `stado market peer list` names them"
            ))),
        }
    }
}

#[async_trait::async_trait]
impl MarketAdapter for PeerAdapter {
    async fn list(&self, offer: ListOffer, json: bool) -> Result<(), CmdError> {
        if offer.price_disk.is_some() || offer.price_min_bid.is_some() {
            return Err(CmdError::usage(
                "--price-disk and --price-min-bid are Vast.ai's prices; a peer fleet is offered \
                 at --price-gpu alone",
            ));
        }
        offers::list(&self.required("list")?, offer.price_gpu, json).await
    }

    async fn unlist(&self, json: bool) -> Result<(), CmdError> {
        offers::unlist(&self.required("unlist")?, json).await
    }

    async fn status(&self, json: bool) -> Result<(), CmdError> {
        offers::status(self.peer.as_deref(), json).await
    }

    // A peer fleet states no income of its own: what this host lent it is
    // what `market status --provider stado` computes from the sessions.
    // Refused, not answered with an empty ledger.
    async fn earnings(&self, _: &str, _: u32, _: bool) -> Result<(), CmdError> {
        Err(CmdError::refused(
            "stado market earnings reads a marketplace's own statement of income, and a Stado \
             peer fleet states none; what this host lent a peer is read with `stado market \
             status --provider stado --peer <peer>`",
        ))
    }

    // A peer fleet's worker drains; there is no renter to tell.
    async fn maintenance(&self, _: u32, _: f64, _: &str, _: bool) -> Result<(), CmdError> {
        Err(CmdError::refused(
            "a Stado peer fleet holds no renter to notify: the lending worker drains when the \
             host stops lending, so take the host out with `stado market unlist --provider stado \
             --peer <peer>` instead",
        ))
    }

    async fn measure(&self, _: &str, _: bool) -> Result<(), CmdError> {
        Err(CmdError::refused(
            "a Stado peer fleet publishes no market of prices; the price a peer is lent at is the \
             one `stado market follow --provider stado --price-gpu` offers",
        ))
    }

    async fn serve_or_rent(&self, _: ServeOrRent<'_>, _: bool) -> Result<(), CmdError> {
        Err(CmdError::refused(
            "serve-or-rent compares with a marketplace's rental price, and a Stado peer fleet \
             publishes none; run it with --provider vast",
        ))
    }

    async fn readiness(
        &self,
        vault_host: Option<String>,
        no_vault_check: bool,
        json: bool,
    ) -> Result<(), CmdError> {
        readiness::report(
            &self.required("readiness")?,
            vault_host,
            !no_vault_check,
            json,
        )
        .await
    }

    async fn verdicts(&self) -> Vec<Value> {
        match every_peer().await {
            Ok(peers) if peers.is_empty() => vec![json!({
                "provider": "stado",
                "ready": false,
                "verdict": "no peer fleet is declared; `stado market peer add` declares one",
            })],
            Ok(peers) => peers
                .into_iter()
                .filter(|(name, _, _)| self.peer.as_ref().is_none_or(|peer| peer == name))
                .map(|(name, ready, verdict)| {
                    json!({"provider": "stado", "peer": name, "ready": ready, "verdict": verdict})
                })
                .collect(),
            Err(error) => vec![json!({
                "provider": "stado",
                "ready": false,
                "verdict": format!("the declared peer fleets cannot be read: {error}"),
            })],
        }
    }

    async fn monitor(&self, bucket: &str, json: bool) -> Result<(), CmdError> {
        offers::monitor(&self.required("monitor")?, bucket, json).await
    }

    async fn follow(&self, follow: Follow) -> Result<(), CmdError> {
        if follow.price_disk.is_some() {
            return Err(CmdError::usage(
                "--price-disk is Vast.ai's price; a peer fleet is offered at --price-gpu alone",
            ));
        }
        run::auto_list(run::Params {
            peer: self.required("follow")?,
            idle_window_s: follow.idle_window_s,
            poll_interval_s: follow.poll_interval_s,
            price_gpu: follow.price_gpu,
            max_duration_s: follow.max_duration_s,
            dry_run: follow.dry_run,
            once: follow.once,
        })
        .await
    }
}
