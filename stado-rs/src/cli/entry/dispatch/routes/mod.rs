//! Where each declared verb lands: one dispatch per declaration block, with
//! the match arms in the order [`crate::cli::entry::spec::root`] declares the
//! variants they answer.

pub(super) mod installation;
pub(super) mod planes;
pub(super) mod platform;
pub(super) mod work;
