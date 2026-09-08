//! The dead-agent reaper pass: delete RUNNING VMs that have stopped doing
//! useful work, and requeue whatever they were holding.
//!
//! [`sweep`] holds the three reap conditions and their age/liveness guards,
//! [`completions`] the completed-refs scan the never-worked condition tests
//! against, and [`race`] the last-moment verdict that tells a genuine listing
//! race from a set of confirmed orphans.

mod completions;
mod race;
mod sweep;

pub use sweep::reap_dead_agents;
