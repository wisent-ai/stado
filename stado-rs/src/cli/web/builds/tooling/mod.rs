//! Running the build's own tools on the builder: one command at a time, the
//! Node toolchain, the locked install, and the commit being cut.

mod command;
pub(in crate::cli::web::builds) mod install;
pub(in crate::cli::web::builds) mod node;
pub(in crate::cli::web::builds) mod revision;
