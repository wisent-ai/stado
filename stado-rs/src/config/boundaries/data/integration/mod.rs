//! Integration boundary constants and shared name checking.

mod clients;
mod endpoints;
mod providers;

pub use clients::*;
pub use endpoints::*;
pub use providers::*;

pub const INTEGRATION_API_VERIFIER_CONSUMER: &str = "stado-integration-api-verifier";
/// Domains reachable through `/api/integration/`. Stado serves only the
/// read-only fleet projection; every product-integration domain moved to the
/// private `wisent-integrations` service together with its client grants.
pub const INTEGRATION_CLIENT_DOMAINS: &[&str] = &["enterprise"];

/// Domains whose provider grant Stado itself resolves. `most` is the SMS
/// escalation path the monitor uses for alerting, so its Twilio credential
/// stays a fleet concern.
pub const INTEGRATION_PROVIDER_DOMAINS: &[&str] = &["most"];

fn canonical_integration_component(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.' | b'_')
        })
}
