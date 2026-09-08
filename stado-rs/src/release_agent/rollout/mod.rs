//! The rollout itself: the candidate, the serving path it is routed onto, the
//! process world both live in, and what the agent does when a candidate
//! refuses.

pub(crate) mod candidate;
pub(crate) mod processes;
pub(crate) mod recover;
pub(crate) mod serving;
