//! The central job record: its fields (`record`) and its behaviour
//! (`methods`).

mod methods;
mod record;

pub use record::{Job, JobSecretRef};
