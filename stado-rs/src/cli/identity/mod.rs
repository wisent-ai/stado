//! Which host holds which identity, and whether that is still true.
//!
//! A compute target is chosen for what it can do. A few kinds of work instead need
//! the one machine that *is* something: the Mac an Apple account is signed into, the
//! host a hardware token is plugged into, the box a licence is bound to. That is not
//! capacity and not permission, so `weles.actions` cannot express it.
//!
//! Three commands, matching the questions an operator and a trajectory ask:
//!
//!   list                   what does the registry claim
//!   verify                 what does each host confirm right now
//!   relay-apple-challenge  capture on the holder and store on the worker
//!
//! `verify` reads the host rather than trusting the declaration, because these
//! identities are granted elsewhere and revoked without notice: an Apple account
//! signs out on a password change, and nothing tells the fleet. A declaration that
//! is never re-checked is the failure mode this module exists to remove -- the flow
//! would otherwise dispatch to a host that stopped qualifying weeks ago and fail
//! deep inside a browser trajectory with a timeout.

mod command;
mod probe;

pub use command::{issue_apple_capabilities, list, relay_apple_challenge, verify};

const APPLE_ACCOUNT: &str = "apple-account";
