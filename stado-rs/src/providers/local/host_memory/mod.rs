//! Declared, automatic management of a host's resident memory.
//!
//! The disk twin is [`crate::providers::local::disk_cleanup`], and this is
//! the same product shape: ONE declaration in the canonical registry
//! (`targets[].memory_reclaim`) names the mode, the watermarks, the per-pass
//! budget and the exact set of permitted repairs; TWO writers execute it —
//! the janitor unit on its own timer and the queue agent's janitor task on
//! every tick; and a host that declares nothing is measured against
//! [`schema::MemoryReclaimPolicy::reporting_default`], which reports and
//! repairs nothing.
//!
//! # Why it exists
//!
//! Memory pressure can prevent services from starting even when disk cleanup
//! is healthy. A shared declaration supplies watermarks, observations and
//! permitted repairs, so automatic action follows recorded policy rather
//! than an ad hoc decision on the affected host.
//!
//! # What a pass may do
//!
//! Only repairs explicitly named by the declaration:
//!
//! | Repair | What it does |
//! | ------ | ------------ |
//! | `restart_unit` | Restarts a declared unit that has no live process. |
//! | `reap_recovery` | Runs a host-recovery program this release already ships. |
//! | `graphical_session` | Ends a declared session process, and only with `allow_graphical_session`. |
//!
//! Refusing the host for new job placement is the fourth permitted effect and
//! is declared as `refuse_placement`. It is applied where both writers of a
//! capacity document pass through
//! ([`crate::queue::capacity::publish_capacity`]), so the host stops being
//! selected while it is over its watermark, with `memory_pressure_active` as
//! the recorded admission reason.

pub mod constants;
pub mod declaration;
pub mod execution;
pub mod reading;
pub mod report;
pub mod state;

// The two halves are re-exported under their own names so that a caller
// naming `host_memory::schema` or `host_memory::pass` still resolves: which
// half a file lives in is this module's business, not its callers'.
pub use declaration::{policies, schema, validate, vocabulary};
pub use execution::{pass, policy, repairs, session};

pub use pass::{run_memory_pass_once, MemoryWriter};
pub use reading::{read_host_memory, MemoryReading};
pub use report::{placement_decision, PlacementDecision, MEMORY_PRESSURE_ACTIVE};
pub use schema::{MemoryReclaimPolicy, MemoryRepairPolicy};
pub use validate::{validate, MemoryPolicyProblem};
