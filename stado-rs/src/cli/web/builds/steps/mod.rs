//! The steps a web product's `.wisent-release.json` recipe runs on a release
//! worker: quality, build, and the post-build smoke test.

mod build;
mod quality;
mod smoke;

pub(crate) use build::build;
pub(crate) use quality::quality;
pub(crate) use smoke::smoke;
