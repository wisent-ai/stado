//! The durable half of the schema: what the pipeline writes down as it runs.

pub(in crate::release_pipeline) mod receipt;
pub(in crate::release_pipeline) mod run;
pub(in crate::release_pipeline) mod worker;
