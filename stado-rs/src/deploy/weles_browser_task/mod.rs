//! Submit one browser task to Weles on a target host, with the action name
//! taken from that host's own allowlist.
//!
//! NO Python original. This module exists because of a gap found on
//! 2026-08-30 while trying to drive a sign-in through Weles on
//! charless-mac-mini. Stado had exactly two ways to put work on a Weles
//! worker and neither could carry an operator's task:
//!
//! - The `weles-capture` workload uses `generic_capture`
//!   ([`super::weles_capture::CAPTURE_ACTION`]), and that action is not in
//!   that host's 226-entry `WELES_ACTION_ALLOWLIST`. The worker refuses any
//!   name outside the allowlist, so that workload cannot run there at all.
//! - The `weles-image-inspect` workload does submit the allowlisted
//!   `generic_browser_task`, but its objective and constraints are fixed in
//!   product code: read-only, no login, no mutation, and an objective about
//!   counting rendered images.
//!
//! So the one action the host would accept was reachable only through a
//! command that could not be told what to do. Every browser workflow this
//! fleet is supposed to own sat behind that.
//!
//! Two properties are deliberate:
//!
//! 1. **The action comes from the host, not from a constant.** The allowlist
//!    is read off the target and the requested action is checked against it
//!    BEFORE any channel is opened, so a name the worker would refuse is
//!    refused here with a sentence naming the action and the host — rather
//!    than enqueued, accepted, and silently dropped. `generic_capture` is
//!    exactly that case and is why this rule exists.
//! 2. **The allowlist is read byte-exact.** `service env-show` clamps every
//!    reported value at 400 characters, and that allowlist is 4488 — reading
//!    it through the diagnostic reader would silently truncate the list to its
//!    first 25 entries and refuse 200 legitimate actions. It is read through
//!    [`super::service_file_fetch`], whose whole contract is that the bytes
//!    arrive unaltered, and the file is never written to disk or printed.

mod action;
mod sign_in;
mod task;

pub use action::{ensure_allowed, host_allowlist, DEFAULT_ACTION, DEFAULT_ALLOWLIST_FILE};
pub use sign_in::{
    exact_origin, host_scopes, issue_sign_in_prefill, routed_item, scope_consumer,
    weles_api_broker_files, AcquisitionScope, RoutedField, SignInPrefill, REGISTERED_SCOPES_FILE,
};
pub use task::{submit, BrowserTask, TaskOutcome};
