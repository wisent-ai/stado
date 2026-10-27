//! Whether one host is open to one peer, at what price, and the lending
//! session that is running or last ran.
//!
//! One document per host and peer under
//! `state/market/offers/<host>--<peer>.json` in this fleet's store. The
//! auto-list loop that holds the peer on the host writes it on every
//! transition; `stado market list` and `unlist` flip `listed`, and the loop
//! reads that switch on its next poll. Every reader — the CLI, Stado
//! Desktop, another host — reads the same document.

use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, Listing, StorageError};

use super::PeerError;

/// Where the offers live in this fleet's store.
pub const OFFERS_PREFIX: &str = "state/market/offers/";

/// One lending session: the worker this host ran against the peer's queue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    /// When the worker was started, and its process id on this host.
    pub started_at: String,
    pub pid: u32,
    /// The worker's own home on this host, removed when the session ends.
    pub home: String,
    /// The longest the session may run, in seconds; none for open-ended.
    pub max_duration_s: Option<i64>,
    /// When and why it ended; none while it runs.
    pub ended_at: Option<String>,
    pub end: Option<String>,
}

/// This host's offer to one peer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub peer: String,
    pub host: String,
    /// The operator's switch: false after `stado market unlist`, true after
    /// `stado market list` or the loop's own idle listing.
    pub listed: bool,
    /// The agreed per-GPU-hour price in USD the peer's seconds are counted
    /// at.
    pub price_gpu: f64,
    /// When the document last changed, and which command changed it.
    pub changed_at: String,
    pub changed_by: String,
    /// What the loop last observed, in its own words.
    pub state: String,
    /// The session running now, or the last one.
    pub session: Option<Session>,
}

fn path(host: &str, peer: &str) -> String {
    format!("{OFFERS_PREFIX}{host}--{peer}.json")
}

/// The offer `host` holds for `peer`, or `None` when it never made one.
pub async fn load(store: &JobStorage, host: &str, peer: &str) -> Result<Option<Offer>, PeerError> {
    let key = path(host, peer);
    let Some(text) = store.download_text(&key).await? else {
        return Ok(None);
    };
    serde_json::from_str(&text).map(Some).map_err(|error| {
        PeerError::NotFound(format!("{key} does not hold an offer record: {error}"))
    })
}

/// Every offer any host of this fleet holds for `peer`.
pub async fn for_peer(store: &JobStorage, peer: &str) -> Result<Vec<Offer>, PeerError> {
    let suffix = format!("--{peer}.json");
    let mut offers = Vec::new();
    for key in store.list_paths(OFFERS_PREFIX, Listing::Every).await? {
        let Some(host) = key
            .strip_prefix(OFFERS_PREFIX)
            .and_then(|rest| rest.strip_suffix(&suffix))
        else {
            continue;
        };
        if let Some(offer) = load(store, host, peer).await? {
            offers.push(offer);
        }
    }
    offers.sort_by(|left, right| left.host.cmp(&right.host));
    Ok(offers)
}

/// Write `offer` and read it back, so the caller reports what the store
/// holds.
pub async fn save(store: &JobStorage, offer: &Offer) -> Result<Offer, PeerError> {
    let body = serde_json::to_string_pretty(offer)
        .map_err(|error| PeerError::Local(format!("the offer cannot be encoded: {error}")))?;
    let key = path(&offer.host, &offer.peer);
    store.upload_text(&key, &body).await?;
    match load(store, &offer.host, &offer.peer).await? {
        Some(stored) => Ok(stored),
        None => Err(PeerError::Storage(StorageError::Other(format!(
            "{key} was written and is absent when read back"
        )))),
    }
}

/// Retire every offer of `peer`, when the peer itself is retired.
pub async fn remove_for_peer(store: &JobStorage, peer: &str) -> Result<(), PeerError> {
    for offer in for_peer(store, peer).await? {
        store.delete_blob(&path(&offer.host, peer)).await?;
    }
    Ok(())
}
