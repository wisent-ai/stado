//! One client: where its endpoint comes from, how its requests are signed,
//! and how an answer is read.

pub(in crate::cli::storage) mod request;
pub(in crate::cli::storage) mod setup;
pub(in crate::cli::storage) mod wire;
