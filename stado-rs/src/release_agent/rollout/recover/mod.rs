//! What the agent does with a refusal: name the run it belongs to, ask that
//! cause's own condition whether the wall still stands, and give the bind
//! back when a candidate has to be taken out.

pub(crate) mod rollback;
pub(crate) mod run;
pub(crate) mod wall;
