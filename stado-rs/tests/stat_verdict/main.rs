//! `stado storage stat` and the five things it can say about one object.
//!
//! The exit-code contract is the whole point: zero means the store ANSWERED —
//! `present` or `absent` — and non-zero means it did not — `refused`,
//! `unavailable`, `unreachable`. A script that reads "is this coordinate
//! spent?" off the exit status therefore cannot mistake a store that could not
//! answer for a drained one, and it branches on `state` for which of the five
//! it got.
//!
//! The area that covered this was deleted rather than repaired: every case
//! rode a hand-written release channel that replied with one canned status
//! line, so no verdict here had ever been computed from a store. This one
//! reaches all five on this machine alone:
//!
//! - [`answered`] asks a filesystem store rooted in the test's own temp
//!   directory, and checks the reported size and version against the bytes and
//!   the digest that are on disk.
//! - [`unanswered`] asks a real loopback HTTP store this test binds and
//!   serves: it hands out the store's own layout marker so the store
//!   constructs, then answers the probed object with a status, or hangs up
//!   without answering at all.
//! - [`preflight`] covers the two refusals that happen before any answer could
//!   exist — a token file that is not owner-only, and a loopback port nothing
//!   listens on.
//!
//! Every sentence and every exit code asserted below was copied from a hand
//! run of the same command on this machine on 2026-09-08.

mod answered;
mod fixture;
mod object_api;
mod preflight;
mod unanswered;
