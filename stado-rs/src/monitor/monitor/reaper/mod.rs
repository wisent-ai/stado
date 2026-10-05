//! The dead-agent reaper pass: delete RUNNING VMs that have stopped doing
//! useful work, and requeue whatever they were holding.
//!
//! [`sweep`] holds the three reap conditions and their liveness guards,
//! [`completions`] the completed-refs scan the never-worked condition tests
//! against, [`measured`] how long VMs of a kind have actually taken to boot
//! and to do their first work, and [`race`] the last-moment verdict that
//! tells a genuine listing race from a set of confirmed orphans.

mod completions;
mod measured;
mod race;
mod sweep;

pub use sweep::reap_dead_agents;
