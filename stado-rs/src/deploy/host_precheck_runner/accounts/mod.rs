//! Every identity a runner presents or is given: the GitHub credential and
//! the calls made with it, the Skarbiec reads behind them, the Brama a runner
//! dials, and the model-review bearer a repository's CI carries.

pub(super) mod brama;
pub(super) mod credentials;
pub(super) mod github;
pub(super) mod model_review;

pub use github::*;
pub use model_review::*;
