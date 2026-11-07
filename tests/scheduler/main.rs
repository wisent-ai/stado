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
mod fenced_store;
#[path = "../host_activity/journey.rs"]
mod host_activity;
#[path = "../host_gates/publication_attempt.rs"]
mod host_gates_publication_attempt;
#[path = "../host_processes/journey.rs"]
mod host_processes;
#[path = "../host_processes/memory.rs"]
mod host_processes_memory;
mod host_routine;
mod lease;
#[path = "../market/peer_declaration.rs"]
mod market_peer;
#[path = "../quality/placed.rs"]
mod quality_placed;
mod readopt;
mod restart;
#[path = "../schedule_overdue/journey.rs"]
mod schedule_overdue;
mod sizing;
#[path = "../workload/omp_session_run.rs"]
mod workload_omp_session;
