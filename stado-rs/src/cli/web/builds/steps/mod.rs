//! The two steps a web product's `.wisent-release.json` recipe runs on a
//! release worker.

mod build;
mod quality;

pub(crate) use build::build;
pub(crate) use quality::quality;
