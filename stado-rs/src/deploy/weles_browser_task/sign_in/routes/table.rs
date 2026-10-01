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
        .ok_or_else(|| DeployError("skarbiec route resolve returned no routes".to_string()))?;
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
        // route reports `does not open: spawn gpg` while the broker service
        // on that same host reads those items fine.
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
