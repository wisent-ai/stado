//! The journeys that run the real `stado serve` over the shared `deployment`
//! fixture, so they compile into one test binary and every fixture item is
//! used by the binary that compiles it. Run one with
//! `cargo test --test scheduler -- <module>::`. A journey whose area is not
//! the scheduler keeps its file under its own area and is named here.
#[path = "../autonomy/policy.rs"]
mod autonomy_policy;
#[path = "../contributions/shared_compute.rs"]
mod contributions;
mod deployment;
#[path = "../host_activity/journey.rs"]
mod host_activity;
mod host_routine;
mod lease;
#[path = "../market/peer_declaration.rs"]
mod market_peer;
mod readopt;
mod restart;
mod sizing;
