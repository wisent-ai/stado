//! The primitives every deploy module is built on: the layer's failure type,
//! the command seam ([`CommandSpec`] / [`CommandOutput`] / [`Runner`]), the
//! production runner's process execution, and the Python-compatible
//! quoting/repr helpers.
//!
//! Everything here is re-exported by `crate::deploy`, which is where callers
//! name it; this module is private and carries no surface of its own.

mod command;
mod error;
mod helpers;
mod process;

pub use command::{production_runner, runner_fn, CommandOutput, CommandSpec, Runner};
pub use error::DeployError;
pub use helpers::{py_dict_repr, py_list_repr, py_str_repr, shlex_quote, write_if_changed};
