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

#[cfg(test)]
mod tests {
    use super::*;

    /// A host with no route for the origin is told which resources and item
    /// disagree, then pointed at the declaration-backed route inspection.
    /// Stado does not choose or mutate a credential route as a side effect.
    #[test]
    fn a_host_without_the_routes_is_told_how_to_inspect_the_declaration() {
        let said = missing_route_sentence(
            "charless-mac-mini",
            "https://accounts.google.com",
            "weles-google-sso-login",
            "no capability route maps origin:https://accounts.google.com/email to a vault field",
        );
        assert!(said.contains("charless-mac-mini"), "{said}");
        assert!(
            said.contains("origin:https://accounts.google.com/email"),
            "{said}"
        );
        assert!(
            said.contains("origin:https://accounts.google.com/password"),
            "{said}"
        );
        assert!(said.contains("weles-google-sso-login"), "{said}");
        // The vault field names a Weles login contract actually uses, the same
        // ones every existing `origin:` route in the fleet maps to.
        assert!(said.contains("username"), "{said}");
        assert!(said.contains("password"), "{said}");
        assert!(
            said.contains("stado route capability weles-admission"),
            "{said}"
        );
        assert!(said.contains("change both mappings in Skarbiec"), "{said}");
    }

    #[test]
    fn an_email_field_class_reads_the_items_username() {
        assert_eq!(vault_field_for("email"), "username");
        assert_eq!(vault_field_for("password"), "password");
    }
}
