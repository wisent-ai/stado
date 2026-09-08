//! The four stages `run.rs` drives, in the order it drives them: which
//! release is current, the install root that release lands in, the
//! environment and Skarbiec identity the unit is given, and whether the unit
//! answers afterwards.
//!
//! Each is one file because each is one host-side conversation with its own
//! remote program, its own marker and its own refusal sentence.

pub(super) mod environment;
pub(super) mod install;
pub(super) mod readiness;
pub(super) mod release;
