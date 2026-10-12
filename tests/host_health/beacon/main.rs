//! The host-health journeys: every one drives an isolated deployment through
//! the shared `deployment` fixture, so they compile into one test binary and
//! every fixture item is used by the binary that compiles it. Run one with
//! `cargo test --test host-health -- <module>::`.
mod beacon;
mod deployment;
mod own_channel;
#[path = "../privacy.rs"]
mod privacy;
mod roles;
mod scans;
