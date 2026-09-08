//! `stado`'s verbs, declared in the four blocks `--help` prints.
//!
//! One `#[derive(Subcommand)]` enum per block, flattened in order by
//! `super::Commands`. Splitting the declaration is a file boundary only:
//! `clap` inserts a flattened enum's variants where the flattening variant
//! stands, so both the accepted command lines and the order they are listed
//! in are the ones the single enum produced.

pub mod installation;
pub mod planes;
pub mod platform;
pub mod work;
