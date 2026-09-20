//! What the agent does with a refusal: name the run it belongs to, ask that
//! cause's own condition whether the wall still stands, retire a refusal the
//! host caused once it may be tried again, and give the bind back when a
//! candidate has to be taken out.

pub(crate) mod retire;
pub(crate) mod rollback;
pub(crate) mod run;
pub(crate) mod wall;
