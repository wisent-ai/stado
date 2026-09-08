//! Everything `stado stream` does by reaching a host: `probe` asks what it
//! could render, `provision` reconciles it to its declaration and records the
//! two services that result, and `session` carries the operations that come
//! after — status, pair and stop.

mod probe;
mod provision;
mod session;

pub(super) use probe::probe;
pub(super) use provision::apply;
pub(super) use session::{pair, status, stop};
