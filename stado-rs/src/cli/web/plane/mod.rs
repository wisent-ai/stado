//! The `web_api` configuration plane, and the parser type over it.
//!
//! Split out of `src/cli/web/mod.rs` so no file in this capability is longer
//! than one sitting can read. The split is mechanical: the command surface,
//! the two configuration-plane helpers, the declaration writer and the
//! inventory each moved whole. `super` re-exports them, so every sibling that
//! reads `super::product`, `super::mutate_web` or `super::WebCommands` still
//! resolves the same items.

mod commands;
mod config;
mod declare;
mod inventory;

pub(crate) use commands::WebCommands;
pub(crate) use config::{mutate_web, product};
pub(crate) use declare::{declare, DeclareRequest};
pub(crate) use inventory::{list, remove};
