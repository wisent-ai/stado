//! A real disk-cleanup pass must never delay a capacity publication.
//!
//! `CAPACITY_STALE_SECONDS` is three times `CAPACITY_HEARTBEAT_INTERVAL_S`,
//! and `release_submit::builder` refuses outright when no fresh publication
//! names the platform, so a host that stops publishing stops being a release
//! builder fleet-wide while staying perfectly healthy. The tick used to
//! `await run_cleanup_once` before publishing, on the same task:
//! charless-mac-mini, 2026-09-03, a `healthy_noop` pass costing 818021 ms
//! against a 300-second interval, and two weles-worker releases refused
//! against a builder that was up.
//!
//! # What is defended here, and where an operator sees it
//!
//! Both cases run the product: `stado agent --target` against an isolated
//! store, with a registry that names THIS machine, and the assertions are read
//! off the capacity documents the agent published and the janitor state file
//! it wrote.
//!
//! - `cases::a_long_cleanup_pass_does_not_delay_the_capacity_publication`
//!   gives the agent a cleaner root big enough that its own pass spends tens
//!   of seconds — measured, and asserted to have spent them — and requires
//!   that publications kept landing while that pass was in flight, at least
//!   one of them strictly inside the pass's own window, with no gap anywhere
//!   near `CAPACITY_STALE_SECONDS`. That is the invariant the incident broke:
//!   the pass is off the publication's critical path.
//! - `cases::the_running_job_count_reaches_the_pass_the_agent_publishes`
//!   claims a real job and requires the count the tick measured to appear in
//!   the pass the janitor persisted and in `diag.disk_cleanup` of the
//!   capacity document the fleet reads. A pass that is told nothing about
//!   running work cannot bound itself against it, and the scheduler that
//!   reads the broadcast would be reading a pass belonging to no host state.
//!
//! # What this area used to do, and no longer does
//!
//! It called `run_cleanup_once` and `JanitorReports::spawn_janitor` in the
//! test process, with a heartbeat scaled to 20 milliseconds and its own
//! stand-in for the tick. Nothing published anything: the "publications" were
//! `Instant`s pushed onto a `Vec` by the test's own loop, and no capacity
//! document, no agent and no command existed anywhere in it.
//!
//! DELETED, with the reason.
//! `the_heartbeat_is_strictly_fresher_than_the_staleness_cutoff` asserted, in
//! a `const` block, that one constant is smaller than another. It runs no
//! product code and there is no command, report or document in which an
//! operator could ever see it fail. The relation it named is not lost: both
//! constants are now the bound
//! `cases::a_long_cleanup_pass_does_not_delay_the_capacity_publication`
//! measures a real publication cadence against, so a change that made the
//! heartbeat slower than the staleness cutoff fails that case with a gap it
//! can print.

mod cases;
mod fixture;
mod fleet;
