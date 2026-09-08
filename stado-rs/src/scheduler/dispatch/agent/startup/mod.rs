//! The startup script a dispatched agent VM boots into: which template a
//! provider gets, the non-secret settings substituted into it, the export
//! contract the template text must satisfy, and the substitution pass.

mod deployment;
mod render;
mod templates;
mod validation;

pub use deployment::deployment_substitutions;
pub use render::{render_agent_startup_script, render_startup_script};
pub(crate) use templates::bundled_template_for;
