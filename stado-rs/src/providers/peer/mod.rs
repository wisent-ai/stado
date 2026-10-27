//! Trading compute with another Stado fleet.
//!
//! A peer is another Stado deployment this fleet has agreed to lend idle
//! capacity to. Its operator gives ours three things: the origin of the
//! object API that serves its queue, the namespace the queue lives in, and a
//! bearer for that namespace, which our operator stores in Skarbiec under a
//! role of their choosing. Nothing else crosses: the peer's jobs are the
//! peer's, recorded in the peer's store, and run by the same open Stado
//! worker every host runs — started on this host as its own process, with
//! its own home, against the peer's queue instead of ours.
//!
//! - [`declaration`]: the peer as this fleet declares it, kept in our store
//!   under `state/market/peers/`.
//! - [`offer`]: whether this host is open to the peer, at what price, and
//!   the lending session in progress, kept under `state/market/offers/`.
//! - [`remote`]: the peer's store opened with its bearer, and what it says
//!   about the work our host ran there.
//! - [`lending`]: one lending session — the worker process, its home, its
//!   configuration and its bearer file — started and ended.
//!
//! Money does not move here. Every job our host ran for the peer is in the
//! peer's own records with its start, completion and the host that ran it;
//! the price this host was offered at turns those seconds into an amount
//! both operators read from the same records.

pub mod declaration;
pub mod lending;
pub mod offer;
pub mod remote;

pub use declaration::{PeerDeclaration, PeerError};
pub use offer::{Offer, Session};

/// The worker kind a lending worker claims under. The peer reads it in its
/// own job records (`worker_allocation.kind`) to tell work a peer fleet ran
/// from work its own hosts ran.
pub const LENDING_KIND: &str = "stado-peer";

/// The field of the item playing a peer's credential role that holds the
/// bearer the peer issued to us.
pub const TOKEN_FIELD: &str = "token";

/// The role lock an auto-list loop holds for `peer` on this host, so one
/// loop lends this host to one peer and `stado market list` can tell
/// whether a loop is there to act on an offer.
pub fn loop_role(peer: &str) -> String {
    format!("market-stado--{peer}")
}
