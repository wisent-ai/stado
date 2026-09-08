//! The route table as the redeeming host answers it: which vault item and
//! field the broker would really hand one resource to.

use serde_json::Value;

use crate::deploy::DeployError;

/// Which vault item the broker would actually hand this resource to.
///
/// The caller names the item it believes holds the account; the route table
/// decides which item is really read. Those two disagreeing is the failure
/// Skarbiec's own route table was built for — a route pointing somewhere the
/// operator did not mean is indistinguishable from a working one until a login
/// needs it. So the claim is checked before a capability exists.
pub fn routed_item(routes: &Value, resource: &str) -> Result<RoutedField, DeployError> {
    let rows = routes
        .get("routes")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("skarbiec routes list returned no routes".to_string()))?;
    let row = rows
        .iter()
        .find(|row| row.get("resource").and_then(Value::as_str) == Some(resource))
        .ok_or_else(|| {
            DeployError(format!(
                "no capability route maps {resource} to a vault field"
            ))
        })?;
    let item = row
        .get("item")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let field = row
        .get("field")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if item.is_empty() || field.is_empty() {
        return Err(DeployError(format!(
            "capability route for {resource} must name an item and a field"
        )));
    }
    Ok(RoutedField {
        item,
        field,
        // Advisory, NOT a gate. `routes list` answers these as the process
        // that asked, and over a host channel that process has no gpg: every
        // route on charless-mac-mini reports `does not open: spawn gpg` while
        // the broker service on that same host reads those items fine.
        // Refusing on them would refuse every real sign-in for the wrong
        // reason, and redemption is where the item is actually read.
        readable: row
            .get("item_present")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && row
                .get("field_present")
                .and_then(Value::as_bool)
                .unwrap_or(false),
    })
}

/// One route as the target answered it.
#[derive(Debug)]
pub struct RoutedField {
    pub item: String,
    pub field: String,
    /// Whether the ASKING process could open the item and find the field.
    /// Advisory only — see [`routed_item`].
    pub readable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A route that does not exist is refused; a route whose readability the
    /// ASKING process could not confirm is reported, not refused. Over a host
    /// channel every route on charless-mac-mini answers `does not open: spawn
    /// gpg` because that session has no gpg, while the broker service on the
    /// same host reads those items fine — so refusing on that boolean would
    /// refuse every real sign-in for a reason that is about the wrong process.
    #[test]
    fn a_missing_route_is_refused_and_an_unconfirmable_one_is_only_reported() {
        let routes = json!({
            "consumer": null,
            "routes": [
                {
                    "resource": "origin:https://accounts.google.com/email",
                    "item": "weles-google-sso-login",
                    "field": "username",
                    "item_present": true,
                    "field_present": true,
                },
                {
                    "resource": "origin:https://accounts.google.com/password",
                    "item": "weles-google-sso-login",
                    "field": "password",
                    "item_present": false,
                    "field_present": false,
                },
                {
                    "resource": "origin:https://dash.cloudflare.com/email",
                    "item": "",
                    "field": "",
                    "item_present": false,
                    "field_present": false,
                },
            ],
        });

        let routed = routed_item(&routes, "origin:https://accounts.google.com/email").unwrap();
        assert_eq!(routed.item, "weles-google-sso-login");
        assert_eq!(routed.field, "username");
        assert!(routed.readable);

        // The `spawn gpg` shape: mapped, but this process could not open it.
        // Still Ok, with readable false for the caller to say out loud.
        let routed = routed_item(&routes, "origin:https://accounts.google.com/password").unwrap();
        assert_eq!(routed.item, "weles-google-sso-login");
        assert_eq!(routed.field, "password");
        assert!(!routed.readable);

        // A table entry that names no coordinates is broken, not advisory.
        let said = routed_item(&routes, "origin:https://dash.cloudflare.com/email")
            .unwrap_err()
            .to_string();
        assert!(said.contains("must name an item and a field"), "{said}");

        // Skarbiec's own sentence for a resource with no route at all.
        let said = routed_item(&routes, "origin:https://example.com/email")
            .unwrap_err()
            .to_string();
        assert_eq!(
            said,
            "no capability route maps origin:https://example.com/email to a vault field"
        );
    }
}
