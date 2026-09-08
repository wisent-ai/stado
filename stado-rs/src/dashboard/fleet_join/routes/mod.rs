//! The three routes a machine reaches while holding nothing but an invite
//! code: the bootstrap script, the public channel key, and the request that
//! files its pending enrollment.

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

mod enrollment;
mod key;
mod report;
mod script;

pub(in crate::dashboard) use enrollment::join;
pub(in crate::dashboard) use key::invite_key;
pub(in crate::dashboard) use script::join_script;
