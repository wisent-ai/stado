//! Real build → release → install proof.
//!
//! These cases drive the compiled `stado` binary end to end. Each creates a
//! clean committed Rust product, starts a real Stado worker against an
//! isolated local store, runs `stado release submit`, requires the worker to
//! execute `cargo check` and `cargo build --release`, signs and publishes the
//! archive with a key read from a real Skarbiec broker, delivers it through
//! `stado release install-local`, then executes the installed binary and
//! checks its version output. No fleet host or operator registry is read or
//! changed.
//!
//! The area is split by what each part defends: [`fixture`] owns the isolated
//! world, [`registry`] the document the control plane parses, [`waits`] the
//! store observations, and the three journeys sit in [`journey`],
//! [`recovery`] and [`retry`].

#[path = "../support/skarbiec.rs"]
mod skarbiec_support;

mod fixture;
mod journey;
mod recovery;
mod registry;
mod retry;
mod waits;
