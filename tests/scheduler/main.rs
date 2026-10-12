//! The scheduler journeys: each runs the real `stado serve` over the shared
//! `deployment` fixture, so they compile into one test binary and every
//! fixture item is used by the binary that compiles it. Run one with
//! `cargo test --test scheduler -- <module>::`.
mod deployment;
mod lease;
mod readopt;
mod restart;
mod sizing;
