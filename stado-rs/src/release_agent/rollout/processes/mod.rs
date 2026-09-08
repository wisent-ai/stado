//! The process world a rollout runs in: what is out there, what the record
//! does not know about, and the proxy an interrupted handoff left behind.

pub(crate) mod inventory;
pub(crate) mod reconcile;
pub(crate) mod sweep;
