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
/// be spent finding that out. The HTTP(S) sentence is the worker's own.
pub fn exact_origin(raw: &str) -> Result<String, DeployError> {
    let parsed = url::Url::parse(raw)
        .map_err(|error| DeployError(format!("--sign-in-origin is not a URL: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(DeployError(
            "credential fill requires an HTTP(S) origin".to_string(),
        ));
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(DeployError(
            "--sign-in-origin must not carry embedded credentials".to_string(),
        ));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(DeployError(
            "credential fill requires an HTTP(S) origin".to_string(),
        ));
    }
    if !matches!(parsed.path(), "" | "/") || parsed.query().is_some() || parsed.fragment().is_some()
    {
        return Err(DeployError(format!(
            "--sign-in-origin must be a bare origin such as https://accounts.google.com, \
             with no path, query or fragment: {raw}"
        )));
    }
    Ok(parsed.origin().ascii_serialization())
}

/// The resource string for one field class on one origin.
pub fn fill_resource(origin: &str, field_class: &str) -> String {
    format!("origin:{origin}/{field_class}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_that_weles_could_never_match_is_refused_before_anything_is_minted() {
        // The worker's own sentence for a non-HTTP(S) page.
        let said = exact_origin("ftp://accounts.google.com")
            .unwrap_err()
            .to_string();
        assert_eq!(said, "credential fill requires an HTTP(S) origin");

        // Weles compares `new URL(page.url()).origin`, which carries no path.
        let said = exact_origin("https://accounts.google.com/signin")
            .unwrap_err()
            .to_string();
        assert!(said.contains("bare origin"), "{said}");
        assert!(said.contains("no path, query or fragment"), "{said}");

        let said = exact_origin("https://user:pw@accounts.google.com")
            .unwrap_err()
            .to_string();
        assert!(said.contains("embedded credentials"), "{said}");

        // A trailing slash is the origin itself and is accepted.
        assert_eq!(
            exact_origin("https://accounts.google.com/").unwrap(),
            "https://accounts.google.com"
        );
        // A non-default port belongs to the origin Weles would compute.
        let ported = "http://localhost:\
                      8080";
        assert_eq!(exact_origin(ported).unwrap(), ported);
    }
}
