//! One fleet-level answer to "can anything claim this queue".
//!
//! NO Python original. The incident it exists for: job `2c4a47aa` sat in
//! `queue/` for 121 hours and no command in this product said why. `stado
//! status` listed it under a row of counts that end in "1 queued"; `stado
//! overview` printed a worker count next to the words "active workers"; and
//! the one fact that explained the stall — that not a single host in the
//! registry currently publishes capacity, so nothing can ever claim it —
//! existed only per-host, one ssh round trip at a time, behind `stado host
//! gates HOST`. An operator who did not already suspect a specific host had
//! no way to reach it.
//!
//! A queue with no claimant looks exactly like an empty queue. This module is
//! the difference, and it is a report: no exit status, no gate, nothing
//! written, nothing deleted.
//!
//! Three sources, joined here and re-derived nowhere:
//!
//! - every host's newest capacity publication, read through
//!   [`capacity::read_publications`] — the GC-free reader, because the
//!   scheduler's [`capacity::read_consumer_capacity`] deletes rows past its
//!   GC horizon and a report that destroys its own evidence would answer
//!   "nobody ever said anything" where the truth is "that host went quiet an
//!   hour ago";
//! - the queued jobs, for the wait that sizes the stall and for the pins that
//!   decide whether a `pinned_only` host is idle by policy or starving;
//! - the registry's declared units joined against the host's newest health
//!   beacon, for the commonest cause of the silence: a declared queue agent
//!   that nothing on the host is running.
//!
//! Every word an operator reads here is [`host_gates`]'s word for the same
//! condition, because a blocker that is greppable in one command and spelled
//! differently in another is two vocabularies for one fact.
//!
//! The three sources are joined in `read`, the words they are reported in sit
//! in `types`, the per-host reasons in `blockers`, and the verdict they add up
//! to in `verdict`.
//!
//! [`capacity::read_publications`]: crate::queue::capacity::read_publications
//! [`capacity::read_consumer_capacity`]: crate::queue::capacity::read_consumer_capacity
//! [`host_gates`]: crate::deploy::host_gates

mod blockers;
mod read;
mod types;
mod verdict;

pub use read::read_fleet_claim;
pub use types::{wait_words, Blocker, HostVerdict, OldestWait};
pub use verdict::FleetClaim;
