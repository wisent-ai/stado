//! Every journey that drives the real service through `fixture::Service`:
//! the desktop API journey, `stado build status` and `stado status` of reaped
//! jobs, cost quotes and cancellation observations. They share one fixture,
//! so they compile into one test binary and every fixture item is used by the
//! binary that compiles it. Run one with
//! `cargo test --test service-journeys -- <module>::`.
#[path = "../build/reaped_jobs.rs"]
mod build_reaped_jobs;
#[path = "../cancellation/journey.rs"]
mod cancellation;
#[path = "../cost/journey.rs"]
mod cost;
mod fixture;
// Loaded by path so its own modules (configuration, console_limits,
// registry_inputs) sit beside it in this directory.
#[path = "journey.rs"]
mod journey;
#[path = "../status/reaped_job.rs"]
mod status_reaped_job;
