//! The command line itself: its failures, its declaration, and its dispatch.
//!
//! [`error`] holds [`CmdError`](error::CmdError) and the exit codes every
//! command answers with; [`spec`] holds the `clap` declaration of the whole
//! command tree; [`dispatch`] holds the process entry point and the match
//! that lands each command on its implementation. [`super`] re-exports all
//! three, so every caller keeps the `crate::cli::<name>` path it already
//! used.

pub mod dispatch;
pub mod error;
pub mod spec;
