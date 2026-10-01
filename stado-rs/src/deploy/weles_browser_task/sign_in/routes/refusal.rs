//! The refusal a host without the routes reads, and the vault field each
//! login field class is kept in.

use super::super::SIGN_IN_FIELDS;
use super::fill_resource;

/// The sentence a caller reads when the redeeming host has no route for the
/// origin they asked to sign in on.
///
/// It names both exact resources and the item they must map to. Route changes
/// belong in Skarbiec; this refusal points at the declaration-backed route
/// inspection rather than mutating which credential a login form receives.
pub fn missing_route_sentence(host: &str, origin: &str, item: &str, detail: &str) -> String {
    format!(
        "{host} cannot fill a sign-in on {origin}: {detail}. It needs both of \
         {} and {}, mapped to vault item {item} fields {} and {}. Inspect the active route with \
         `stado route capability weles-admission`, change both mappings in Skarbiec if needed, \
         then run this again.",
        fill_resource(origin, SIGN_IN_FIELDS[0].1),
        fill_resource(origin, SIGN_IN_FIELDS[1].1),
        vault_field_for(SIGN_IN_FIELDS[0].1),
        vault_field_for(SIGN_IN_FIELDS[1].1),
    )
}

/// The vault field a Weles login contract keeps each class in.
///
/// Skarbiec's own login shape, and the one every `origin:` route in the fleet
/// already uses: `platform-admin-cloudflare/username` and `/password`,
/// `platform-admin-appstore/username` and `/password`. An email field class
/// reads the item's `username`.
pub fn vault_field_for(field_class: &str) -> &'static str {
    match field_class {
        "email" => "username",
        _ => "password",
    }
}
