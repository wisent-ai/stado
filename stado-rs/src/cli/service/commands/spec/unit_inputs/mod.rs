//! The two blocks of `stado service` verbs that change what a running unit
//! reads: its environment, and its credentials.
//!
//! Both are flattened into [`super::super::ServiceCommands`] in declaration
//! order, so grouping them here changes neither the accepted command lines
//! nor the order `--help` lists them in.

pub mod credentials;
pub mod environment;
