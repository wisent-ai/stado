//! The published service value of a tunnel's catch-all ingress rule.

/// Cloudflare keeps one hostname-less rule last in every tunnel configuration.
/// Both ingress edits restore exactly this service when the rule list has none,
/// so the value is named once here instead of inside either branch.
pub(super) const CATCH_ALL_SERVICE: &str = "http_status:404";
