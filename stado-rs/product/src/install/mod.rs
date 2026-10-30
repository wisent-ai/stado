mod after_install;
mod command;
pub use after_install::{declares, redeclare};
pub use command::run;
pub(crate) mod handoff;
pub mod operation;
pub use operation::perform;
pub mod plan;
pub(crate) mod policy;
mod recipes;
mod services;
mod settle;
pub mod status;
mod sweep;
mod transaction;
use anyhow::{Context, Result};
use serde_json::Value;

pub fn recipe<'a>(product: &'a Value, surface: &str) -> Result<&'a Value> {
    product["installations"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["surface"] == surface))
        .with_context(|| format!("{} has no installation recipe for {surface}", product["id"]))
}

/// What an installation is bound to beyond the catalog's recipe: an exact
/// qualified release, or an exact canonical commit a source build exports.
#[derive(Clone, Copy)]
pub enum Coordinate<'a> {
    Release { version: &'a str, revision: &'a str },
    SourceCommit(&'a str),
}
