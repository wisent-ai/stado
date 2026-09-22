//! How a release run ends: finished by its own last job, or superseded by a
//! newer submission.

pub(in crate::cli::release_submit) mod finish;
pub(in crate::cli::release_submit) mod supersede;
