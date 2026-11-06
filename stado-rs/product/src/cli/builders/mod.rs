//! The `stado product` subcommands each owning module declares: the catalog,
//! the registry, names and the installation lifecycle.

pub(super) mod catalog;
pub(super) mod lifecycle;
pub(super) mod name;
pub(crate) mod registry;

use super::{flag, positional, value};
