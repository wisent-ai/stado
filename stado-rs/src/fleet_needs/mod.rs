//! What the fleet lacks, from evidence: the unmet-placement ledger every
//! refusing path writes, and the advisor that reads it beside the capacity
//! publications, the beacons and the registry to say which host needs RAM,
//! which needs disk, whether a GPU or a whole platform is missing.
//!
//! `unmet` is the record. `advisor` turns records and readings into needs.
//! `render` prints them.

pub mod advisor;
pub mod render;
pub mod unmet;

pub use advisor::{advise, Evidence, Need, NeedKind, NeedsReport, Severity};
pub use unmet::{
    read_unmet, record_unmet, this_requester, Candidate, Requirement, UnmetPlacement, UnmetReason,
};
