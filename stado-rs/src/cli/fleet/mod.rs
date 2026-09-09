//! `stado fleet` — enrollment, fleet membership, SSH-key custody and worker
//! diagnosis for the registered Stado hosts.
//!
//! This is the whole implementation behind the main CLI. Adding a machine
//! used to live outside that CLI, which meant the command an operator needs
//! first was invisible to `stado --help` and to anything built on its public
//! surface. Keeping the parser and dispatch here makes the advertised command
//! and its implementation one path.
//!
//! The fleet's blind spot before `doctor` existed: a worker could sit in a
//! crash loop with no command able to say why. `doctor` closes that — it
//! verifies the agent credential grant against the configured allowlist,
//! probe-reads every declared secret field without printing values, and
//! reports per-target beacon and capacity presence, all through Stado's own
//! reads.

mod command;

pub mod doctor;
pub mod enroll;
pub mod fleets;
pub mod ingress;
pub mod invite;
pub mod key;
pub mod ops;

pub use command::{run, FleetCommands, IngressCommands, KeyCommands};
