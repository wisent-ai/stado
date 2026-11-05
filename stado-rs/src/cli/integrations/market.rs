//! `stado market`: one adapter per marketplace behind [`MarketAdapter`], and
//! the dispatch that answers the provider-less verbs first and sends every
//! other verb to the adapter `--provider` names.
//!
//! Each verb is a method every adapter answers: with the marketplace's own
//! operation, or with a refusal saying why that marketplace has none. A new
//! marketplace is a catalog row in the `market` capability family, a
//! [`MarketProvider`] value, and one implementation of this trait; the
//! commands, `stado capabilities market` and Stado Desktop reach it without
//! another change.

use serde_json::Value;

use crate::cli::{CmdError, MarketCommands, MarketProvider};

/// The prices `list` offers a host at.
pub(crate) struct ListOffer {
    /// Per-GPU-hour price in USD.
    pub price_gpu: f64,
    /// Per-GB-month disk price in USD, for a marketplace that prices disk.
    pub price_disk: Option<f64>,
    /// Interruptible-bid floor, for a marketplace that takes bids.
    pub price_min_bid: Option<f64>,
    /// How many units the offer sells, for a marketplace that sells a
    /// counted amount (HandToHuman).
    pub units: Option<i64>,
    /// Hours ahead the offer ends, for a marketplace whose offers carry an
    /// end date (Vast.ai).
    pub contract_hours: Option<u32>,
}

/// What `follow` holds the host's listing to.
pub(crate) struct Follow {
    pub idle_window_s: i64,
    pub poll_interval_s: Option<u64>,
    pub price_gpu: f64,
    pub price_disk: Option<f64>,
    pub max_duration_s: i64,
    /// Hours ahead the offer ends, moved forward while the host stays idle
    /// (Vast.ai).
    pub contract_hours: Option<u32>,
    pub dry_run: bool,
    pub once: bool,
}

/// What `serve-or-rent` compares.
pub(crate) struct ServeOrRent<'a> {
    pub target: &'a str,
    pub outside_model: &'a str,
    pub gpu: &'a str,
    pub since_hours: u32,
}

/// One marketplace a host can be offered on.
#[async_trait::async_trait]
pub(crate) trait MarketAdapter: Send + Sync {
    /// Offer the host at `offer`.
    async fn list(&self, offer: ListOffer, json: bool) -> Result<(), CmdError>;
    /// Withdraw every offer of the host; rentals already running are kept.
    async fn unlist(&self, json: bool) -> Result<(), CmdError>;
    /// The marketplace's view of the host: offered or not, and its rentals.
    async fn status(&self, json: bool) -> Result<(), CmdError>;
    /// What the marketplace states the host earned per day.
    async fn earnings(&self, target: &str, since_days: u32, json: bool) -> Result<(), CmdError>;
    /// Tell the host's renters it goes down for maintenance.
    async fn maintenance(
        &self,
        in_minutes: u32,
        hours: f64,
        category: &str,
        json: bool,
    ) -> Result<(), CmdError>;
    /// What renters pay for a board model now.
    async fn measure(&self, gpu: &str, json: bool) -> Result<(), CmdError>;
    /// Whether serving a model earns more than renting its board here.
    async fn serve_or_rent(&self, compared: ServeOrRent<'_>, json: bool) -> Result<(), CmdError>;
    /// Whether the host can be offered here at all, with the refusing rule.
    async fn readiness(
        &self,
        vault_host: Option<String>,
        no_vault_check: bool,
        json: bool,
    ) -> Result<(), CmdError>;
    /// The readiness rows `stado capabilities market` prints for this
    /// marketplace: `{"provider", "ready", "verdict"}` and any key naming
    /// what a row is about.
    async fn verdicts(&self) -> Vec<Value>;
    /// One snapshot of the marketplace's view and the queue.
    async fn monitor(&self, bucket: &str, json: bool) -> Result<(), CmdError>;
    /// Offer the host while our queue is idle and withdraw it when work
    /// arrives.
    async fn follow(&self, follow: Follow) -> Result<(), CmdError>;
}

/// The adapter of `provider`; `peer` is the declared Stado fleet a
/// `--provider stado` verb names and `resource` what a `--provider
/// handtohuman` verb offers, each refused for every other marketplace.
pub(crate) fn adapter(
    provider: MarketProvider,
    peer: Option<&str>,
    resource: Option<super::handtohuman::Resource>,
) -> Result<Box<dyn MarketAdapter>, CmdError> {
    if peer.is_some() && provider != MarketProvider::Stado {
        return Err(CmdError::usage(
            "--peer names a Stado fleet and is taken only with --provider stado",
        ));
    }
    if resource.is_some() && provider != MarketProvider::HandToHuman {
        return Err(CmdError::usage(
            "--resource names what a HandToHuman offer sells and is taken only with --provider \
             handtohuman; Vast.ai and a peer fleet are offered whole boards",
        ));
    }
    match provider {
        MarketProvider::Vast => Ok(Box::new(super::vast::VastAdapter)),
        MarketProvider::Stado => Ok(Box::new(super::peer::PeerAdapter {
            peer: peer.map(str::to_string),
        })),
        MarketProvider::HandToHuman => Ok(Box::new(super::handtohuman::HandToHumanAdapter {
            resource,
        })),
    }
}

/// Dispatch one `market` subcommand.
pub(crate) async fn dispatch(command: &MarketCommands) -> Result<(), CmdError> {
    if let MarketCommands::Peer(peer) = command {
        return super::peer::declare(peer).await;
    }
    if let MarketCommands::Node(node) = command {
        return super::node::dispatch(node).await;
    }
    // The source list, payouts and the comparison span every marketplace,
    // so no provider is named.
    if let Some(answered) = super::vast::provider_less(command).await {
        return answered;
    }
    match command {
        MarketCommands::List {
            provider,
            peer,
            resource,
            price_gpu,
            price_disk,
            price_min_bid,
            units,
            contract_hours,
            json,
        } => {
            let offer = ListOffer {
                price_gpu: *price_gpu,
                price_disk: *price_disk,
                price_min_bid: *price_min_bid,
                units: *units,
                contract_hours: *contract_hours,
            };
            adapter(*provider, peer.as_deref(), *resource)?
                .list(offer, *json)
                .await
        }
        MarketCommands::Unlist {
            provider,
            peer,
            resource,
            json,
        } => {
            adapter(*provider, peer.as_deref(), *resource)?
                .unlist(*json)
                .await
        }
        MarketCommands::Status {
            provider,
            peer,
            resource,
            json,
        } => {
            adapter(*provider, peer.as_deref(), *resource)?
                .status(*json)
                .await
        }
        MarketCommands::Earnings {
            provider,
            peer,
            resource,
            target,
            since_days,
            json,
        } => {
            adapter(*provider, peer.as_deref(), *resource)?
                .earnings(target, *since_days, *json)
                .await
        }
        MarketCommands::Maintenance {
            provider,
            peer,
            in_minutes,
            hours,
            category,
            json,
        } => {
            adapter(*provider, peer.as_deref(), None)?
                .maintenance(*in_minutes, *hours, category, *json)
                .await
        }
        MarketCommands::Measure {
            provider,
            peer,
            gpu,
            json,
        } => {
            adapter(*provider, peer.as_deref(), None)?
                .measure(gpu, *json)
                .await
        }
        MarketCommands::ServeOrRent {
            provider,
            peer,
            target,
            outside_model,
            gpu,
            since_hours,
            json,
        } => {
            let compared = ServeOrRent {
                target,
                outside_model,
                gpu,
                since_hours: *since_hours,
            };
            adapter(*provider, peer.as_deref(), None)?
                .serve_or_rent(compared, *json)
                .await
        }
        MarketCommands::Readiness {
            provider,
            peer,
            vault_host,
            no_vault_check,
            json,
        } => {
            adapter(*provider, peer.as_deref(), None)?
                .readiness(vault_host.clone(), *no_vault_check, *json)
                .await
        }
        MarketCommands::Monitor {
            provider,
            peer,
            bucket,
            json,
        } => {
            // `--bucket` names the store when given; else it is the one the
            // configuration selects.
            let bucket = match bucket.as_deref() {
                Some(named) => named,
                None => crate::config::bucket(),
            };
            adapter(*provider, peer.as_deref(), None)?
                .monitor(bucket, *json)
                .await
        }
        MarketCommands::Follow {
            provider,
            peer,
            idle_window_s,
            poll_interval_s,
            price_gpu,
            price_disk,
            max_duration_s,
            contract_hours,
            dry_run,
            once,
        } => {
            let follow = Follow {
                idle_window_s: *idle_window_s,
                poll_interval_s: *poll_interval_s,
                price_gpu: *price_gpu,
                price_disk: *price_disk,
                max_duration_s: *max_duration_s,
                contract_hours: *contract_hours,
                dry_run: *dry_run,
                once: *once,
            };
            adapter(*provider, peer.as_deref(), None)?
                .follow(follow)
                .await
        }
        MarketCommands::Peer(_)
        | MarketCommands::Node(_)
        | MarketCommands::Sources { .. }
        | MarketCommands::Payout(_)
        | MarketCommands::Compare { .. }
        | MarketCommands::Decisions { .. } => {
            unreachable!("the provider-less verbs were answered above")
        }
    }
}
