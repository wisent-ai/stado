//! The sign-in journey: the purpose, target, window and field pair Weles
//! itself derives its expectation from, and the broker instance a fill for
//! that expectation is issued into.

use crate::deploy::host_capability;

mod prefill;
mod routes;
mod scopes;

pub use prefill::{issue_sign_in_prefill, SignInPrefill};
pub use routes::{exact_origin, routed_item, RoutedField};
pub use scopes::{host_scopes, scope_consumer, AcquisitionScope, REGISTERED_SCOPES_FILE};

/// The capability purpose a browser field fill redeems under.
///
/// Weles derives the expectation itself as
/// `{ purpose: 'weles.browser.fill', resource: "origin:<page origin>/<field
/// class>" }` and refuses anything else before it redeems, so these two
/// constants are not a convention this module chose — they are the worker's.
pub const FILL_PURPOSE: &str = "weles.browser.fill";

/// The capability target Weles requires on a reference it will redeem.
pub const CAPABILITY_TARGET: &str = "weles";

/// Skarbiec's own maximum, and what the Apple sign-in asks for. A browser run
/// is held open for its whole duration, so a shorter window would expire
/// mid-flow; single use is what makes the exposure one fill.
pub const SIGN_IN_TTL_SECONDS: &str = "3600";
pub const SIGN_IN_MAX_USES: &str = "1";

/// The pair a form sign-in needs: the fill target handed to Weles, and the
/// field class that target must agree with.
///
/// The targets are not decoration. Weles refuses a fill whose target does not
/// match the field class's own hint — `/email|e-mail/` and
/// `/password|passcode|secret/` — before redeeming, so a pair that disagreed
/// would burn a one-shot capability on `credential field class mismatch`.
pub const SIGN_IN_FIELDS: [(&str, &str); 2] = [("email", "email"), ("password", "password")];

/// The capability state the Weles API's own broker serves.
///
/// From that product's launcher, `launch-weles-api-mac.sh:
/// 104`: the broker on
/// `$HOME/.stado/run/weles-api-capability.sock` — the socket the worker's
/// `SKARBIEC_CAP_SOCKET` names — is started with these files, not with
/// Skarbiec's vault-adjacent defaults. A capability issued into the default
/// state is invisible to it.
pub const WELES_API_CAPABILITY_FILE: &str = "$HOME/.stado/weles-api-capabilities.json";

/// The route table that same broker resolves against
/// (`launch-weles-api-mac.sh:
/// 105`).
///
/// Note for anyone declaring a route here: that launcher REINSTALLS this file
/// from `weles/scripts/worker/deploy/weles-capability-routes.json` on every
/// start, so a route declared on the host lasts until the unit next launches.
/// The durable place for a new one is that checked-in file.
pub const WELES_API_ROUTES_FILE: &str = "$HOME/.stado/weles-api-capability-routes.json";

/// The broker instance a Weles browser fill is issued into.
pub fn weles_api_broker_files() -> host_capability::BrokerFiles<'static> {
    host_capability::BrokerFiles {
        capability_file: Some(WELES_API_CAPABILITY_FILE),
        routes_file: Some(WELES_API_ROUTES_FILE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fill targets must satisfy Weles's own field-class hints, or the
    /// worker throws `credential field class mismatch` and the one-shot
    /// capability is already spent.
    #[test]
    fn the_fill_targets_match_the_hints_weles_checks_before_redeeming() {
        let hints = [("email", "email"), ("password", "password")];
        for ((target, field_class), (expect_target, expect_class)) in
            SIGN_IN_FIELDS.iter().zip(hints)
        {
            assert_eq!(*target, expect_target);
            assert_eq!(*field_class, expect_class);
            assert!(target.to_lowercase().contains(field_class));
        }
    }
}
