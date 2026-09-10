//! The release run itself: the source identity a submission is made of, the
//! durable run object it maintains, the orchestrator that walks it, and the
//! reports read back from it.

pub(in crate::cli::release_submit) mod reports;
pub(in crate::cli::release_submit) mod resume;
pub(in crate::cli::release_submit) mod source;
pub(in crate::cli::release_submit) mod state;
pub(in crate::cli::release_submit) mod submit;
