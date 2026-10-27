//! The stable part of the automation surface: the envelope version, the
//! error type, the canonical encodings and the machine-facing job view.

pub(in crate::machine) mod cancellation;
pub(in crate::machine) mod encoding;
pub(in crate::machine) mod error;
pub(in crate::machine) mod jobs;

/// The machine envelope's version, stated with its compatibility policy:
/// <https://stado.wisent.com/docs/machine-interface>.
pub const SCHEMA_VERSION: i64 = 1;
