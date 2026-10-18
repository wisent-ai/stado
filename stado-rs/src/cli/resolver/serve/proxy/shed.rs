//! Ending held relays from the resolver's own process.
//!
//! A wait stays in the published list for as long as its client holds the
//! connection: no clock cuts it. Two things end it from this side. One is
//! the listener running out of file descriptors, which `accept` reports as
//! its own error: every held connection is a descriptor the resolver keeps
//! for a request nothing answers, and an adapter that cannot accept serves
//! nobody at all, so every wait older than the refresh interval is ended.
//! The other is the session the wait rides being replaced because it stood
//! unanswering while a new session to the same host answered: nothing will
//! answer the wait on the standing session, and left there it would hold its
//! caller without end.
//! Each relay listens on an ender here; told to end, it refuses its client
//! with the resolver's sentence, gives its place back and frees its
//! descriptor, and the client's own request fails with that cause.

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
pub(in crate::cli::resolver::serve) struct Shed {
    pub(in crate::cli::resolver::serve) ended: Vec<WaitingOpen>,
    pub(in crate::cli::resolver::serve) unsheddable: usize,
}

/// Tell the relay of every wait in `waits` to end with `reason`; a wait whose
/// relay listens on no ender (a channel still opening) is counted unsheddable.
fn end(waits: Vec<(u64, WaitingOpen)>, reason: &str) -> Shed {
    let enders = enders();
    let (ended, unsheddable): (Vec<_>, Vec<_>) = waits.into_iter().partition(|(key, _)| {
        enders
            .get(key)
            .is_some_and(|ender| ender.send(Some(reason.to_string())).is_ok())
    });
    Shed {
        ended: ended.into_iter().map(|(_, open)| open).collect(),
        unsheddable: unsheddable.len(),
    }
}

/// End every wait older than `refresh_seconds` with `reason`.
pub(super) fn shed_held(refresh_seconds: u64, reason: &str) -> Shed {
    end(waiting::session::older_than(refresh_seconds), reason)
}

/// End every wait on adapter `bind` to `endpoint` on `host` that rides a
/// replaced session, with `reason`.
pub(in crate::cli::resolver::serve) fn shed_replaced(
    bind: &str,
    host: &str,
    endpoint: &str,
    reason: &str,
) -> Shed {
    end(waiting::session::on_replaced(bind, host, endpoint), reason)
}

/// Whether an accept error is the process out of file descriptors: the one
/// accept failure the resolver answers by shedding instead of ending.
pub(super) fn out_of_descriptors(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(code) if code == libc::EMFILE || code == libc::ENFILE)
}
