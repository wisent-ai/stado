//! Ending held relays from the resolver's own process.
//!
//! A wait stays in the published list for as long as its client holds the
//! connection: no clock cuts it. One thing does end it from this side: the
//! listener running out of file descriptors, which `accept` reports as its
//! own error. Every held connection is a descriptor the resolver keeps for a
//! request nothing answers, and an adapter that cannot accept serves nobody
//! at all. So the relay of each wait listens on an ender here, and when
//! `accept` fails for want of descriptors the resolver sends the reason to
//! every wait older than its refresh interval; each relay refuses its client
//! with that sentence, gives its place back and frees its descriptor, and
//! the adapter goes on accepting.

use std::collections::BTreeMap;
use std::sync::Mutex;

use nix::libc;
use tokio::sync::watch;

use crate::cli::resolver::report::waiting::{self, WaitingOpen};

/// What a relay is told when its wait is ended from this side: the
/// resolver's own sentence, written to the client as the refusal.
pub(super) type Ender = watch::Sender<Option<String>>;

static ENDERS: Mutex<BTreeMap<u64, Ender>> = Mutex::new(BTreeMap::new());

/// The register, recovered from a poisoned lock: it holds senders only, so
/// a panic while it was held leaves nothing half-written.
fn enders() -> std::sync::MutexGuard<'static, BTreeMap<u64, Ender>> {
    match ENDERS.lock() {
        Ok(enders) => enders,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Let the wait under `key` be ended from this side.
pub(super) fn listen(key: u64, ender: Ender) {
    enders().insert(key, ender);
}

/// The wait under `key` is over; its ender goes with it.
pub(super) fn forget(key: u64) {
    enders().remove(&key);
}

/// What one shed did: the waits whose relays were told to end, and how
/// many held waits had no relay to tell (a channel still opening stands on
/// the SSH session, not on a relay).
pub(super) struct Shed {
    pub(super) ended: Vec<WaitingOpen>,
    pub(super) unsheddable: usize,
}

/// End every wait older than `refresh_seconds` with `reason`.
pub(super) fn shed_held(refresh_seconds: u64, reason: &str) -> Shed {
    let enders = enders();
    let mut ended = Vec::new();
    let mut unsheddable = Vec::new();
    for (key, open) in waiting::session::older_than(refresh_seconds) {
        match enders.get(&key) {
            Some(ender) if ender.send(Some(reason.to_string())).is_ok() => ended.push(open),
            _ => unsheddable.push(open),
        }
    }
    Shed {
        ended,
        unsheddable: unsheddable.len(),
    }
}

/// Whether an accept error is the process out of file descriptors: the one
/// accept failure the resolver answers by shedding instead of ending.
pub(super) fn out_of_descriptors(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(code) if code == libc::EMFILE || code == libc::ENFILE)
}
