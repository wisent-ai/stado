//! Scans of a host's own disk — the janitor's build-cache walk, the beacon's
//! launchd event read, the `du` and `find` inventory a space report runs —
//! share one admission, one priority and one record.
//!
//! The installed host process walked the whole home directory on every
//! health pass, at the operator's own I/O priority, with the machine already
//! loaded by the agents working on it: the pass added to a load it could
//! have read. A scan is now admitted only while the one-minute load average
//! is under the host's ceiling (`host.scan_load_ceiling`, or the logical
//! core count this process measures when nothing is declared), runs at
//! background I/O priority, and a skipped scan is recorded here so the
//! beacon publishes it and `stado host health` shows it, instead of the
//! scan silently not happening.

pub mod admission;
pub mod priority;
pub mod record;

pub use admission::{admit, Admission, Verdict};
pub use priority::{background_prefix, Throttled};
pub use record::{published, Skip};
