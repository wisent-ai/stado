//! What the checkout declares about the product being built: the worker
//! contract the step runs under, the product's `package.json`, what its
//! `.wisent-release.json` states, and the names the artifact carries.

pub(in crate::cli::web::builds) mod naming;
pub(in crate::cli::web::builds) mod package;
pub(in crate::cli::web::builds) mod release;
pub(in crate::cli::web::builds) mod worker;
