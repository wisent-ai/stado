//! Which vault this machine resolves to, whether any key still opens it, and
//! what survives in agent transcripts when nothing does.

pub(in crate::cli::secrets) mod doctor;
pub(in crate::cli::secrets) mod harvest;
pub(in crate::cli::secrets) mod unlock;
