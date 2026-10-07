//! The coordinates a fill is expected on: the page origin in the exact form
//! Weles compares against, and the resource string one field class takes on
//! that origin.

use crate::deploy::DeployError;

mod confirm;
mod refusal;
mod table;

pub use confirm::confirm_routed_item;
pub use table::{routed_item, RoutedField};

/// One page origin, in the exact form Weles compares against.
///
/// Weles builds its expectation from `new URL(page.url()).origin`, so anything
/// carrying a path, a query, a fragment or userinfo could never match and would
/// be spent finding that out. The HTTP(S) sentence is the worker's own; the
/// others name `sign_in_origin`, the plan field an operator can actually edit,
/// because `stado workload run weles-browser-task --plan FILE` is the only
/// surface that reaches here and no command takes a `--sign-in-origin` flag.
pub fn exact_origin(raw: &str) -> Result<String, DeployError> {
    let parsed = url::Url::parse(raw).map_err(|error| {
        refused_origin(format!(
            "weles-browser-task plan sign_in_origin is not a URL: {error}"
        ))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(refused_origin(
            "credential fill requires an HTTP(S) origin".to_string(),
        ));
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(refused_origin(
            "weles-browser-task plan sign_in_origin must not carry embedded credentials"
                .to_string(),
        ));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(refused_origin(
            "credential fill requires an HTTP(S) origin".to_string(),
        ));
    }
    if !matches!(parsed.path(), "" | "/") || parsed.query().is_some() || parsed.fragment().is_some()
    {
        return Err(refused_origin(format!(
            "weles-browser-task plan sign_in_origin must be a bare origin such as \
             https://accounts.google.com, with no path, query or fragment: {raw}"
        )));
    }
    Ok(parsed.origin().ascii_serialization())
}

/// A plan `sign_in_origin` this command cannot fill credentials for: the
/// operator's plan is refused.
fn refused_origin(message: String) -> DeployError {
    DeployError(message).stating(crate::primitives::failure::FailureCode::Refused)
}

/// The resource string for one field class on one origin.
pub fn fill_resource(origin: &str, field_class: &str) -> String {
    format!("origin:{origin}/{field_class}")
}
