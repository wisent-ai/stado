//! The central job record: its fields (`record`) and its behaviour
//! (`methods`).

mod methods;
mod record;
mod worker;

pub use record::{Job, JobSecretRef};
pub use worker::{WorkerAllocation, WorkerResource};
