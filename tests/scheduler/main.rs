//! The journeys that run the real `stado serve` over the shared `deployment`
//! fixture, so they compile into one test binary and every fixture item is
//! used by the binary that compiles it. Run one with
//! `cargo test --test scheduler -- <module>::`. A journey whose area is not
//! the scheduler keeps its file under its own area and is named here.
#[path = "../contributions/shared_compute.rs"]
mod contributions;
mod deployment;
mod lease;
mod readopt;
mod restart;
mod sizing;
