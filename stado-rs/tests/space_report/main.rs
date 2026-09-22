//! `stado space report`, driven as the real binary against an isolated
//! registry that names this machine, so the read executes locally.
//!
//! What this defends is one shape of failure. The report is two things at
//! once: three cheap fields — free space, the janitor's last outcome, memory —
//! and one attribution walk over the whole selected tree. The walk cost more
//! than the shared two-minute channel bound, so on 2026-09-02 and again on
//! 2026-09-09 the command died having computed nothing, on the very machine
//! whose disk was the question. The cheap fields cost under a second and were
//! lost with it.
//!
//! So the walk now has its own budget, and exceeding it is reported rather
//! than fatal. A budget of zero is that same branch without a race: it says
//! the walk was not attempted, so the cheap report an operator needs on a
//! host whose walk costs minutes is one environment value away, and the
//! branch is provable on a warm machine instead of only on a slow one.


mod fixture;

// The cases live beside this file rather than in it: a test binary root may
// declare its modules from anywhere, and grouping them keeps each folder
// readable.
#[path = "cases/cleaners.rs"]
mod cleaners;
#[path = "cases/paging.rs"]
mod paging;
#[path = "cases/refusals.rs"]
mod refusals;
#[path = "cases/report.rs"]
mod report;
#[path = "cases/walk.rs"]
mod walk;
