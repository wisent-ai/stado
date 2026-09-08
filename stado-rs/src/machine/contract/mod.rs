//! The stable part of the automation surface: the envelope version, the
//! error type, the canonical encodings and the machine-facing job view.

pub(in crate::machine) mod encoding;
pub(in crate::machine) mod error;
pub(in crate::machine) mod jobs;

pub const SCHEMA_VERSION: i64 = 1;
