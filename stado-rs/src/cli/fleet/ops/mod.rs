//! Fleet write operations: `create` and `assign`.
//!
//! Every write is a pure document-to-document transform followed by a
//! compare-and-swap against the generation the transform's own input was read
//! at — `create`, `delete` and `assign` through `commit_document`, which
//! re-reads and re-applies the transform when another writer got there first.
//! `enroll` cannot: it installs a key and probes the machine between its read
//! and its write, so re-applying its transform would republish a decision
//! taken against a host that has since been described differently. It takes
//! one conditional attempt and lets the conflict reach the operator.
//!
//! One component per verb: `declare` creates a fleet, `retire` removes one,
//! `assignment` moves a target into one and `enrollment` onboards a machine,
//! rollback included.

mod assignment;
mod declare;
mod enrollment;
mod retire;

pub use assignment::{assign, assign_target};
pub use declare::{create, create_fleet};
pub use enrollment::{enroll, preflight_enroll, register_target, remove_target};
pub use retire::{delete, delete_fleet};
