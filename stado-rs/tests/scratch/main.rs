//! The scratch capability, driven against a real machine.
//!
//! Every test here runs the built `stado` binary and ends by reading state on a
//! host: an account the directory service answers for, a record in a login
//! account's home, a home directory, or the absence of all three. Nothing in
//! this area simulates a host, and nothing asserts a plan.
//!
//! Three rules the whole area obeys:
//!
//! - **The host comes from the fleet.** `stado scratch hosts --json` says which
//!   registry targets a lease may be taken on and which profile covers each.
//!   No test names a machine, and none reads a host out of the environment.
//! - **Only what the test named is touched.** A lease is created under a name
//!   this run generated, and destroyed by name. The canonical registry is read
//!   and never written; the operator's own accounts are never candidates,
//!   because `host_user_delete` refuses them by name.
//! - **A run that cannot reach a host fails.** If the registry does not answer
//!   or no target is leasable, the test fails carrying the fleet's own words.
//!   It never falls back to a tempdir and reports success.
//!
//! Run it the way an operator runs the CLI — with the fleet's configuration in
//! the environment:
//!
//! ```sh
//! STADO_API_URL=https://<coordinator> STADO_CONFIG=~/.config/stado/config.json \
//!   cargo test --test scratch
//! ```

#[path = "../support/owned_home.rs"]
mod owned_home;

mod fleet;
mod lifecycle;
mod refusals;
