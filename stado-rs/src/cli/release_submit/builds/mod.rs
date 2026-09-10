//! Platform build jobs: which live fleet builder may claim one, the queue
//! plan that carries its immutable request, and the worker that runs it.

pub(in crate::cli::release_submit) mod builder;
pub(in crate::cli::release_submit) mod claimability;
pub(in crate::cli::release_submit) mod jobs;
pub(in crate::cli::release_submit) mod scratch;
pub(in crate::cli::release_submit) mod worker;
