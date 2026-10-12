//! The resolver's held-connection journeys: each drives the shared
//! `held_deployment` fixture, so they compile into one test binary and every
//! fixture item is used by the binary that compiles it. Run one with
//! `cargo test --test service-directory-held -- <module>::`.
mod held;
mod held_answer;
mod held_deployment;
mod self_loop;
