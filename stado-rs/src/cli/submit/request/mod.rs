//! Request assembly: the kwargs map a profile is folded into, and the
//! command that turns the resolved flags into one durable submission.

pub(in crate::cli::submit) mod dispatch;
mod kwargs;
