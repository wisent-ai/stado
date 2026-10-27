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
        .ok_or_else(|| {
            DeployError::unreachable("skarbiec route resolve returned no routes".to_string())
        })?;
    let row = rows
        .iter()
        .find(|row| row.get("resource").and_then(Value::as_str) == Some(resource))
        .ok_or_else(|| {
            DeployError(format!(
                "no capability route maps {resource} to a vault field"
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    // The route names its item and field as text; a row missing either, or
    // carrying it in another shape, is a route that cannot be redeemed.
    let text = |member: &str| match row.get(member).and_then(Value::as_str) {
        Some(value) if !value.is_empty() => Ok(value.to_string()),
        _absent_blank_or_not_text => Err(DeployError(format!(
            "capability route for {resource} must name an item and a field; its {member} is {}",
            row[member]
        ))
        .stating(crate::primitives::failure::FailureCode::Config)),
    };
    let item = text("item")?;
    let field = text("field")?;
    // Advisory, NOT a gate. `routes list` answers these as the process
    // that asked, and over a host channel that process has no gpg: every
    // route reports `does not open: spawn gpg` while the broker service
    // on that same host reads those items fine.
    // Refusing on them would refuse every real sign-in for the wrong
    // reason, and redemption is where the item is actually read. A flag the
    // row does not carry is read as not shown readable.
    let present = |member: &str| matches!(row.get(member), Some(Value::Bool(true)));
    Ok(RoutedField {
        item,
        field,
        readable: present("item_present") && present("field_present"),
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
